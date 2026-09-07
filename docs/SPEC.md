# gitgui: specification

## 1. Goal

A git GUI comparable in feel to Sourcetree or GitKraken's core panels (graph, changes, diff, stage by hunk, commit, branch switching), rendered as pixels inside an existing terminal pane. Instant startup, a few MB binary, works over SSH, zero browser engine.

Target terminals: Ghostty and cmux first, kitty second. Anything with kitty graphics + SGR pixel mouse should work.

Non-goals: an interactive rebase editor (single-commit rewrites are offered from the commit menu instead), bisect, submodules, worktrees, Windows.

## 2. Architecture

```
                  stdin bytes                 Event channel
  Terminal  ─────────────────►  term::input  ───────────────►  main loop
     ▲                                                             │
     │  APC G frames (shm or base64)                               │ iced events
     │                                                             ▼
  term::kitty  ◄──────  render::frame  ◄──────  shell::Shell (iced UserInterface + tiny-skia)
                         (RGBA, dirty check)     builds App::view, updates, draws into the buffer
                                                                           │
                                                                           │ reads RepoSnapshot, emits Command
                                                                           ▼
                                                              git worker thread (git2)
                                                              produces new RepoSnapshot
```

Threads:

- **input thread**: blocking `read(2)` on stdin, pushes raw bytes to a channel. The main loop drains the channel, feeds `term::input::Parser`, gets `Vec<Event>`.
- **main loop**: waits on input channel or a redraw deadline, hands the events to `shell::Shell`, which builds the iced `UserInterface` from `App::view`, delivers the events, applies the resulting `Message`s to `App`, rebuilds while messages keep coming, draws with the tiny-skia renderer straight into the framebuffer and sends the frame if it changed.
- **git worker**: receives `Command`, executes with `git2` or the `git` CLI, sends back `RepoSnapshot` or `OpResult`. The UI never blocks on git.

Frame pacing: render only when iced asks for a redraw (`RedrawRequest::NextFrame` or `At`, for cursor blink and animations) or an event arrived. Idle CPU must be zero.

### 2.1 Rendering (shell.rs)

iced 0.14 without a window: `iced_runtime::user_interface::UserInterface` plus `iced_renderer::Renderer` with only the `tiny-skia` backend. One frame:

1. `UserInterface::build(app.view(), size, cache, renderer)`, then `operate` for queued focus operations.
2. `update(events, cursor, renderer, clipboard, messages)`; key presses no widget captured become `Message::Key` for the app's single-key bindings.
3. If messages came out, apply them to `App` and rebuild (at most four rounds); the UI that produced no new messages is drawn.
4. `draw`, then `Renderer::draw` into a `tiny_skia::PixmapMut` over the framebuffer with a reused clip mask, and an R/B swap (tiny-skia keeps BGRA, the kitty encoder wants RGBA).

Layout is an iced `pane_grid` (drag a title bar to move a pane, drag the gaps to resize, maximize / restore). The commit log, the diff and the merge tool are custom widgets that lay out and draw only their visible rows, so a 2000 commit log or a 10k line diff costs nothing off screen.

tiny-skia facts that shape the custom widgets (see PLAN, "iced branch"): per-text clip rects are honored only when the rect pokes outside the current layer, nested layers do not intersect, a geometry group's clip rect gets the layer translation applied twice, and text widgets consult their key bindings even when unfocused.

Performance targets: 1600x1000 frame at scale 2 under 8 ms in release, 2400x1500 under 16 ms.

### 2.2 Framebuffer (render/frame.rs)

Two `Vec<u8>` RGBA buffers of `w*h*4`. After drawing, compare against the last sent buffer (memcmp, it is fast); send only if different. Export to PNG for `--headless-frame`.

### 2.3 Input mapping to iced (shell.rs)

- Key press / release -> `keyboard::Event::KeyPressed / KeyReleased { key, modified_key, physical_key, location, modifiers, text, repeat }`. Named keys map to `keyboard::key::Named`, characters carry the shifted text; `text` is dropped for ctrl / alt combos and control characters.
- Mouse press / release -> `CursorMoved` then `ButtonPressed / ButtonReleased`, motion -> `CursorMoved`, wheel -> `WheelScrolled { delta: Lines }` (positive y scrolls up, like winit). Positions are `pixel / pixels_per_point`.
- Focus -> `window::Event::Focused / Unfocused`. Resize -> new logical size on the next build.
- Paste (bracketed) is stored in the shell clipboard and delivered as a synthetic Ctrl+V so the focused text widget takes it through its own paste path. Copy requests from widgets and the app go out as `OSC 52 ; c ; <base64> ST`.

## 3. Git layer (git/)

### 3.1 RepoSnapshot

Immutable, cheaply clonable (Arc). Rebuilt by the worker after any command and on a filesystem poll every 2 s while focused (stat `.git/HEAD`, `.git/index`, `.git/refs` mtime and the working tree top level; cheap and good enough for v1).

```
RepoSnapshot {
  path, head: HeadInfo { branch_name, oid, detached },
  branches: Vec<Branch { name, oid, is_remote, upstream, ahead, behind, is_head }>,
  tags: Vec<Tag>, stashes: Vec<Stash { index, message, oid }>, remotes: Vec<String>,
  commits: Vec<CommitRow>,          // topo order, newest first, capped at 2000 with "load more"
  graph: GraphLayout,               // see 3.3
  unstaged: Vec<FileStatus>, staged: Vec<FileStatus>, conflicted: Vec<FileStatus>,
}
CommitRow { oid, short, parents: Vec<Oid>, summary, author, email, time, refs: Vec<RefLabel> }
FileStatus { path, old_path (renames), kind: Added|Modified|Deleted|Renamed|Untracked|TypeChange }
```

### 3.2 Commands

```
Refresh | SetDiffOpts { context, ignore_whitespace }
Stage(paths) | Unstage(paths) | StageAll | UnstageAll | Discard(paths) | DiscardAll   // discards ask in the UI
StageHunk | UnstageHunk | DiscardHunk { path, hunk_index }
StageLines | UnstageLines | DiscardLines { path, hunk_index, lines }
Ignore(pattern)
Commit { message, amend } | CommitAndPush { message, amend }
Checkout(branch) | ForceCheckout | StashAndCheckout | CheckoutDetached(oid)
CreateBranch { name, from, checkout } | DeleteBranch | RenameBranch | SetUpstream | FastForward
Merge(branch) | CherryPick(oid) | Revert(oid) | Reset { oid, kind: Soft|Mixed|Hard }
Resolve { path, side: Ours|Theirs }
CreateTag { name, oid, message } | DeleteTag
StashPushOpts { message, keep_index, include_untracked } | StashPop | StashApply | StashDrop | BranchFromStash
RemoteAdd | RemoteRemove | RemoteRename | RemoteSetUrl
// git CLI, output streamed to the log panel:
Fetch | FetchRemote | Pull | PullRebase | Push | ForcePush | PushTag | DeleteRemoteBranch | PublishGithub
Rebase(onto) | RewriteCommit { oid, action: Drop|Squash|Fixup|Reword|Edit|MoveUp|MoveDown, message } | Autosquash
State { action: Continue|Abort|Skip, subcommand }                            // merge, rebase, cherry-pick, revert
LoadDiff { target: WorkdirUnstaged(path) | Staged(path) | Commit(oid, path) }
LoadCommitFiles(oid)
```

Implementation notes with `git2`:

- Status: `StatusOptions` with `include_untracked(true)`, `recurse_untracked_dirs(true)`, `renames_head_to_index(true)`, `renames_index_to_workdir(true)`.
- Stage: `index.add_path` / `index.remove_path` for deleted files, then `index.write()`. Untracked directories: `index.add_all` with the path spec.
- Unstage: reset the index entry to HEAD's tree entry (`repo.reset_default(Some(&head_obj), paths)`).
- Hunk staging: get the diff `diff_index_to_workdir` for the path, then `repo.apply(&diff, ApplyLocation::Index, options)` with `ApplyOptions::hunk_callback` returning true only for the selected hunk index. Unstage hunk: `diff_tree_to_index` for the path, `Diff` reversed via `diff.reverse` isn't available in git2, so build the reverse patch text with `Patch::to_buf`, flip it (swap +/-, swap old/new headers), parse with `Diff::from_buffer`, and apply to the index. Test this thoroughly with fixtures.
- Commit: signature from `repo.signature()`, tree from `index.write_tree()`, parents from HEAD (or none for the initial commit), `repo.commit(Some("HEAD"), ...)`. Amend: `head_commit.amend(...)`. Respect `commit.gpgsign` only by shelling out to `git commit` when it is true (git2 cannot sign without extra setup).
- Diff text: `Patch::from_diff` per file, iterate hunks and lines, keep `origin` (`+`, `-`, ` `, header) and old/new line numbers. Detect binary and cap files above 2 MB with a "too large" message.
- Log: `Revwalk` with `Sort::TOPOLOGICAL | Sort::TIME`, push all local branch heads and HEAD (option to include remotes). Store parents per row.
- Ahead/behind: `repo.graph_ahead_behind(local, upstream)`.
- Network ops: `Command::new("git").args([...])` with `GIT_TERMINAL_PROMPT=0`, stdout/stderr streamed to the UI log. Never hang on a prompt.
- Line staging: `actions::partial_patch` rebuilds one hunk with only the selected lines as changes (forward for staging; reversed, with unselected additions kept as context, for unstaging and discarding), then `repo.apply` to the index or the working tree. `\ No newline at end of file` is carried per line.
- Cherry-pick, revert and merge use libgit2's own operations, then commit the result with the original author (cherry-pick) or a generated message. libgit2 leaves the result in the in-memory index and its safe checkout neither creates nor deletes files, so the index is written and the touched paths are synced from the new HEAD (`actions::finish_pick`). Conflicts stay in the index with the operation state set; the CLI's `--continue` finishes them.
- History rewriting: `git rebase -i <oid>~1` (or `--root`) with `GIT_SEQUENCE_EDITOR="gitgui --sequence-editor"` and `GIT_EDITOR="gitgui --commit-editor"`. The editor subprocess reads the action, the commit and an optional message from `GITGUI_TODO_*` environment variables and rewrites the todo (`git/rebase.rs`). Squash into below and move down rebase from two commits below. Only commits on the first-parent chain below HEAD without a merge in between are offered, and only on a clean tree.
- Conflicts: `resolve_conflict` writes the chosen stage's blob (or deletes the file) and re-adds the path. A conflicted file's "diff" is the working tree file with the ours block as removals and the theirs block as additions (`repo::conflict_view`).
- The cached index is re-read before every write (`Repo::index()`): other processes write it all the time.

### 3.3 Commit graph layout (git/graph.rs)

Input: commit rows in topo order. Output per row: `lane: usize`, `edges: Vec<Edge { from_lane, to_lane, kind: Straight|Merge|Fork }>`, `color: usize`.

Algorithm (gitk style):

```
active: Vec<Option<Oid>>   // lane -> commit expected next in that lane
for each commit c (newest first):
  matches = lanes where active[lane] == c.oid
  lane = matches.first() or first free slot (None) or push new lane
  for each other lane in matches: emit Edge(other -> lane, Merge into c), set active[other] = None
  if c.parents is empty: active[lane] = None
  else:
    active[lane] = parents[0]
    for p in parents[1..]:
      if some lane l already expects p: emit Edge(lane -> l, Fork)
      else: l = first free slot or new lane; active[l] = p; emit Edge(lane -> l, Fork)
  color[lane] assigned when a lane starts, cycle through the palette
  trim trailing None lanes
```

Draw lanes as vertical lines, edges as quarter-circle curves between rows, commits as filled circles, merges as hollow circles. Column width 14 pt per lane, max 12 lanes visible then clip with a fade.

Unit test with a fixture DAG: linear history, one merge, one octopus, two independent roots.

## 4. UI (ui/)

Layout, an iced `pane_grid` with five panes. Every title bar drags its pane (the bar tints and the pointer becomes a hand over it), the gaps resize, the arrows glyph maximizes or restores (also `1` .. `5`), the x hides the pane and the footer grows a `+ <pane>` button to bring it back next to its usual neighbour:

State file: the pane layouts (main and editor), the maximized pane, hidden panes, collapsed sidebar sections, wrap toggles, open tree folders, diff context and whitespace options and the commit list's column widths are written as JSON to `<gitdir>/gitgui.json` (`ui/state.rs`) whenever they change, once the pointer rests, and on quit; the next start of gitgui in that repository restores them. The file lives in the git directory so it never shows up as an untracked file. Unknown pane ids, duplicate panes or bad ratios make gitgui fall back to the default layout; a missing field takes its default. `GITGUI_NO_STATE=1` neither reads nor writes it.

Desktop window mode (`window.rs`): with `--window`, when stdin is not a tty, or when the probe finds no kitty graphics or a multiplexer, main runs the same `App` through `iced::application` in a native window (winit, softbuffer, the tiny-skia renderer; no wgpu). Git replies and agent jobs cross into the program as `Message::External` through one subscription fed by a futures channel; ignored key presses become `Message::Key` through `keyboard::listen`; window resize, pointer position and modifiers arrive as messages so `App::window`, `cursor` and `modifiers` stay filled; a thread ticker runs at 500 ms only while toasts are visible or the state file is dirty. `App::pending` goes to the worker after each message, `App::ops` become widget tasks, `pending_copy` goes to the system clipboard, `quit` exits the program. Not available in the window: `--split`, the cmux preview, agent screenshots (the frame buffer is the window's), terminal palette colors (the dark theme is used). The window icon comes from `assets/logo.png` (`ui/logo.rs`, decoded with the `png` crate); macOS ignores window icons, so `scripts/bundle-macos.sh` builds `gitgui.app` with an icns made from the same file.

Open another repository: `Ctrl+O`, the `Change folder` footer button, or the one on the not-a-repository screen (which shows the logo from `assets/logo.png`, `ui/logo.rs`) opens `Modal::OpenFolder`: a path field, `..`, and the visible subfolders of that path with a `git` tag on those holding a `.git`; a row descends, Enter or Open sends `Command::Open(path)`. The worker reopens at the new path (snapshot, or `NoRepo` for a plain folder, whose screen offers init or another folder); `App::open_repository` drops everything tied to the old repository (selection, diff, editor, merge tool, tree, filter, commit box), flushes the old state file and loads the new repository's. The agent socket keeps the path it was started with.

```
┌ Repository ───┬ Commits ──────────────────────────────────────────┐
│ ▾ Local       │ Commit | Author | Date header, dividers drag        │
│    * main     │ commit list (graph | refs + summary | author | age)│
│ ▾ Remote      ├ Changes ──────────────┬ Diff ─────────────────────┤
│ ▾ Tags        │ conflicts, unstaged,  │ diff, editor, or the       │
│ ▾ Stashes     │ staged, commit box    │ three-way merge tool       │
├ Files ────────┤ or the commit's files │                            │
│ tree          │                       │                            │
└───────────────┴───────────────────────┴────────────────────────────┘
footer: name + version | branch switcher, ahead/behind, counts, merge banner, last op | + hidden panes | fetch pull push refresh help quit
```

A file opened from the tree, and the merge tool, switch to a second layout: Repository and Files on the left, the editor or the tool taking the rest. Closing restores the five panes. Sidebar sections collapse from the arrow in their header. The commit list's Author and Date columns are resized by dragging the dividers in its header row.

Layout rules learned the hard way:

- Text that must not wrap gets `Wrapping::None`; a text that must give way gets a `container(...).width(Fill).clip(true)`. A `column` that hands `FillPortion` heights to children must itself be `height(Fill)`.
- Overlays (menus, dialogs, toasts) draw inside their own render layer (`widgets::layered`) so they sit above the custom widgets' clip layers.
- The footer drops its key hints below 1000 pt; the commit buttons sit on their own row so a narrow Changes pane keeps them.

Behaviors:

- Click a branch: select its tip in the list. Double click or `Enter`: checkout. Right click: checkout, new branch from here, rename, delete, merge into current, rebase current onto it, fast-forward, set / unset upstream, open pull request, copy name. Remote branches add checkout detached, set as upstream, delete on remote. Remotes: fetch, edit URL, rename, remove, plus an `Add remote` button. Tags: checkout detached, new branch, push to a remote, delete, plus `New tag at HEAD`. Stashes: apply, pop, branch from stash, drop.
- Click a commit: load files and diff for the first file. Refs render as colored pills before the summary. Right click: new branch, tag, checkout detached, cherry-pick, revert, reset (soft / mixed / hard), reword, squash, fixup, drop, move up / down, edit, create fixup commit, autosquash, copy hash / message, open in browser. Rewrites are enabled only for commits the current branch can rebase (`App::rewrite_info`).
- Files: a `Files` section under Stashes lists the whole working tree, not only changed files. Directories are listed lazily by the git worker (`Command::ListDir`), `.git` is skipped, ignored entries are dimmed, changed files take their status color and a collapsed folder with changes shows a dot. Click a file to open it in the built-in editor; right click: edit, open in `$EDITOR`, preview in cmux, show changes, stage, copy path.
- Editor (`ui/editor.rs`): replaces the diff pane while open. Plain `TextEdit` with a line-number gutter and the hand-written highlighter in `ui/highlight.rs` (comments, strings, numbers, keywords, types for the common languages, picked from the extension). `Ctrl+S` saves with the file's original line endings and triggers a refresh; `Escape` closes, asking first when the buffer is dirty (save and close, discard, cancel). A clean editor follows the file selection; a dirty one stays. Files over 1 MB or binary are refused with a hint to use `Shift+E`. `--open <path>` opens a file at startup (also for headless frames).
- Editor layout: a file opened from the change lists (or `e` on one) switches the main area to three columns: sidebar, a column with the commit list above the file lists (working tree lists and commit box, or the commit's files), and the editor at full height; commit rows narrower than 420 pt drop the author column. A file opened from the sidebar file tree hides the commit column too: sidebar and editor only. Closing the editor restores the two-row layout.
- Working tree row: unstaged and staged lists side by side; click a file to show its diff; click the `+` / `-` icon or press `s` / `u` to stage or unstage; `Stage all` / `Unstage all` / `Discard all` buttons; `Discard` with a confirmation modal. Right click a file: stage / unstage, discard, add to .gitignore, copy path; conflicted files offer use ours / use theirs / mark resolved.
- Diff view: monospace, line numbers for old and new, colored backgrounds for + and - lines, hunk headers with `Stage hunk` / `Unstage hunk` and `Discard hunk` buttons, horizontal scroll, word-wrap toggle. Click, Shift+click or drag lines to select them; the header then offers `Stage N lines` / `Unstage N lines` / `Discard N lines` (also `s` / `u` / `d`). `Ctrl+F` searches with match highlighting, `n` / `Shift+N` step; `{` / `}` change context lines, `Ctrl+W` toggles whitespace. Syntax highlighting in the diff is out of scope; the editor has it.
- Commit box: multiline text edit, `Ctrl+Enter` commits, amend checkbox, shows the author from config.
- Search: `/` focuses a filter box over the commit list (summary, author, short hash).
- Footer: while a merge, rebase, cherry-pick or revert is in progress a red banner names it (with rebase progress) and offers `Continue` and `Abort`; `m` opens the same choices plus `Skip`.
- Dialogs: `?` lists every shortcut (`ui/help.rs` is the single source for the table). Destructive commands (drop, reset hard, discard all, force push, delete on remote, abort) always confirm first.
- Toast notifications for op results, errors in red with the git stderr text.
- Panels: every panel header has a small `hide` button, the footer has one toggle per panel (sidebar, commits, detail) and `1` / `2` / `3` toggle from the keyboard. A hidden log or detail pane gives its space to the other; with both hidden the main area shows a hint. Tab skips hidden panes.
- Everything must be operable with mouse only and with keyboard only.

### Keybindings

Single keys and Ctrl combos the terminal does not claim:

```
j / k, Down / Up      move selection          Enter          open / checkout
s / u                 stage / unstage file or selected lines
Space                 toggle staged           a / Shift+A    stage all / unstage all
d / Shift+D           discard file or lines / discard everything (both ask)
i                     ignore untracked file   c              focus commit message
e / Shift+E / Shift+O edit file (built-in) / open in $EDITOR / cmux file preview
Ctrl+S                save in the editor      Escape         close the editor (asks when dirty)
Ctrl+Enter            commit                  Ctrl+Shift+Enter  commit and push
Shift+S               stash (with options)    /              filter commits
Ctrl+F, n / Shift+N   search diff, next / previous match
{ / }                 diff context            Ctrl+W         ignore whitespace
n / Shift+T           new branch / tag at the selected commit
Shift+C / t / g       cherry-pick / revert / reset at the selected commit
Shift+R / d           reword / drop the selected commit
Shift+K / Shift+J     move the selected commit up / down
y / o                 copy hash / open commit in browser
m                     continue, abort or skip a merge or rebase
f / p / Shift+P       fetch / pull / push     r              refresh
Tab                   cycle focus between sidebar, list, detail
1 / 2 / 3             hide or show the sidebar / commit list / detail pane
Escape                clear line selection, diff search, filter; close dialog
?                     help                    q, Ctrl+C      quit
```

`d` means "drop" when a commit is selected and "discard" on the working tree row. `n` steps through search matches while a diff search is active.

Single keys are ignored while a text field has focus; `Ctrl+Enter`,
`Ctrl+Shift+Enter` and `Escape` still work there. `Ctrl+Shift+Enter` is the
one `Ctrl+Shift` binding we claim; terminals do not use it.

### Theme

Dark default. Query the terminal background (PROTOCOLS section 5); if it is light, use the light theme. Fonts: Fira Sans (bundled by iced) for the UI, the system monospace face through cosmic-text for code; the size follows the terminal cell height, `--font-size` overrides it.

## 5. CLI

```
gitgui [path]                     open repo at path (default: discover from cwd)
  --split right|left|down|up            open in a new terminal split (see section 6)
  --size 0.2..0.95                      fraction of the pane the split takes
  --scale 1|1.5|2                       override pixels_per_point
  --font-size N
  --open path                           open a file in the built-in editor at startup
  --editor cmd                          editor for Shift+E (then git config gitgui.editor, $GITGUI_EDITOR, $VISUAL, $EDITOR, vi)
  --probe                               print terminal capabilities and exit
  --headless-frame out.png [--size WxH] render one frame to PNG and exit (used by tests and by the agent)
  --dump-input                          print decoded events
  --no-shm                              force the direct transport
```

Exit codes: 0 ok, 2 not a git repository, 3 terminal lacks kitty graphics (print which terminals are supported), 4 inside tmux/zellij without passthrough.

## 6. Split integration (split.rs)

Detect the host:

- cmux: `CMUX_*` environment variables or `TERM_PROGRAM=cmux`. cmux ships a CLI for pane control; check `cmux --help` at build time and use its split command, passing the current binary path and arguments. Verify in a real cmux session, do not guess flag names.
- Ghostty: `TERM_PROGRAM=ghostty`. Ghostty does not expose a stable CLI for splits from a child process at the time of writing. Try in this order: the `ghostty` binary's `+action` support if present in the installed version, otherwise print the keybinding hint and run in place.
- kitty: `kitty @ launch --location=vsplit --cwd=current <argv>` when remote control is enabled.
- `Shift+E` on a file reuses the same integration to open the user's editor in a new split to the right: `cd <workdir> && <editor> <path>`. The editor is `--editor`, then `git config gitgui.editor`, then `$GITGUI_EDITOR`, `$VISUAL`, `$EDITOR`, then `vi`. GUI editors (`code`, `cursor`, `subl`, `zed`, `mate`, `idea` and friends, matched on the command's basename) are spawned detached instead of in a split. gitgui keeps running. Ghostty gets a toast with the command line instead.
- `Shift+O` runs `cmux open <file>`: cmux's own file preview tab (rendered markdown, syntax colors) in the pane gitgui runs in. Only offered when cmux is detected.

Fallback: run in the current pane and print one line explaining why.

## 7. Agent control API (agent.rs, phase 5)

Mirror terminal-browser's `action` idea so a coding agent in the neighboring pane can drive the GUI. Unix socket at `$XDG_RUNTIME_DIR/gitgui/<pid>.sock` (macOS: `$TMPDIR`), JSON lines:

```
{"cmd":"status"}                      -> snapshot summary (branch, counts, selected commit)
{"cmd":"select","oid":"abc123"}
{"cmd":"stage","paths":["a.rs"]}      {"cmd":"unstage",...}
{"cmd":"commit","message":"..."}
{"cmd":"screenshot","path":"/tmp/x.png"}
{"cmd":"list"}                        // list open instances (answered by any instance via a directory scan)
```

`gitgui action <json>` and `gitgui ls` are the CLI front ends. Ship a `skill/SKILL.md` describing the API for agents, same as terminal-browser does.

## 8. Milestones

Each phase ends with tests green, clippy clean, a headless PNG reviewed, and a manual check in Ghostty or cmux.

**Phase 0: terminal plumbing.**
`term/mod.rs`, `term/probe.rs`, `term/kitty.rs`. `--probe` prints capabilities. Interactive mode paints a solid color image filling the pane with a moving 40x40 square, quits on `q`. Manual check: no flicker, clean exit, panic restores the terminal, works over `ssh localhost`.

**Phase 1: rendering.**
`render/raster.rs`, `render/frame.rs`. egui demo with text, buttons, a scroll area. `--headless-frame` works. Unit tests: single triangle pixel count, clip rect, textured quad with a 2x2 texture. Manual check: text is crisp at scale 1 and 2, 60 fps while dragging a slider.

**Phase 2: input.**
`term/input.rs` complete with tests for every sequence in PROTOCOLS section 3. Mouse clicks, drag, wheel, keyboard, paste, focus, resize all drive the egui demo. `--dump-input` works.

**Phase 3: read-only git.**
`git/repo.rs`, `git/graph.rs`, sidebar, log with graph, commit detail, diff view. Test graph layout on fixture DAGs. Test repo module against a temp repository created with `git2` in tests. Manual check: open this project's repo and the linux kernel checkout (performance).

**Phase 4: writes.**
Stage/unstage files and hunks, commit, amend, checkout, branch create/delete, stash, discard with confirmation, fetch/pull/push via CLI with a log panel. Tests for hunk staging round trips. Manual check: full commit workflow without touching the git CLI.

**Phase 5: integration.**
`split.rs`, `agent.rs`, `skill/SKILL.md`, install script (`curl | bash` that downloads a release binary for macOS arm64/x86_64 and Linux x86_64/arm64), GitHub Actions release workflow, README with a demo recording.

## 9. Testing without a graphics terminal

- Protocol encoders and parsers: byte-exact unit tests.
- Rendering: `--headless-frame` PNG, inspected visually and asserted by sampling a few known pixels in tests (e.g. background color at (0,0), a button's fill color at its center).
- Git: temp repos built in tests, covering staged/unstaged/untracked/renamed/deleted/conflicted states.
- End to end: a `scripts/smoke.sh` that runs `--probe`, `--headless-frame`, and a scripted session via the agent socket against a fixture repo.

## 10. Suggested Cargo.toml

```toml
[package]
name = "gitgui"
version = "0.4.0"
edition = "2021"

[dependencies]
iced_core = "=0.14.0"
iced_runtime = "=0.14.0"
iced_widget = { version = "=0.14.2", default-features = false, features = ["canvas"] }
iced_renderer = { version = "=0.14.0", default-features = false, features = ["tiny-skia", "fira-sans", "geometry"] }
tiny-skia = { version = "=0.11.4", default-features = false, features = ["std", "simd"] }
git2 = { version = "=0.21.0", default-features = false }
libc = "0.2"
base64 = "0.22"
flate2 = "1"
png = "0.17"
anyhow = "1"
serde = { version = "1", features = ["derive"] }   # agent API only
serde_json = "1"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
```

Pin exact versions and record the iced API notes in CLAUDE.md; the `iced` umbrella crate is not used because it drags in winit.
