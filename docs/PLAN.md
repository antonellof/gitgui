# gitgui plan and status

Living document. Updated at the end of every phase and whenever an issue is
found or closed. The milestones themselves are defined in SPEC.md section 8;
this file tracks where we are, what was decided, and what is open.

## Status

| Phase | State | Commit | Notes |
|---|---|---|---|
| 0 terminal plumbing | done | 22bef03 | probe, raw mode, kitty graphics shm + direct, restore on exit and panic |
| 1 rendering | done | 475c593 | rasterizer, framebuffer, egui demo, headless PNG |
| 2 input | done | eecddce | parser with byte tests, stdin thread, egui mapping, `--dump-input` |
| 3 read-only git | done | 9b7de3d | git2 0.21.0, snapshot + graph + diff, worker thread, real UI |
| 4 writes | done | b4de578 | stage/unstage files and hunks, commit, amend, checkout, branches, stash, discard, fetch/pull/push via git CLI |
| 5 integration | done | b394bdc | split.rs, agent.rs, skill/SKILL.md, install script, release workflow |
| post-v0.1 polish | done | ccdf818 | v0.1.1 to v0.1.4: keyboard shortcuts, agent network ops, auto refresh, footer toolbar, branch picker, GitHub publish |
| review pass | done | f38d9a3 | 2026-09-04 audit: layout fixes for short and narrow panes, socket permissions, bounds checks; see "Review 2026-09-04" |
| feature parity pass | done | a2c3498 | 2026-09-05: line staging, diff search / context / whitespace, commit menu, history rewriting, merge / rebase state and conflicts, branch / remote / tag / stash operations, help; see "Feature audit 2026-09-05" |
| editor and file tree | done | (this commit) | 2026-09-05: built-in editor with syntax colors (`ui/editor.rs`, `ui/highlight.rs`), `Shift+E` external editor in a split with `--editor` / `gitgui.editor`, `Shift+O` cmux file preview, sidebar file tree (`ui/tree.rs`, lazy `Command::ListDir`), `--open` |

## Measurements

Release build, raster stage only, `--headless-frame`:

| Frame | Time |
|---|---|
| 1600x1000 scale 1 | 2 ms |
| 1600x1000 scale 2 | 2.7 ms |
| 3344x1870 scale 2 (full cmux pane on the dev Mac) | 10 ms |

On screen in cmux (includes shm copy, encode, dirty check): about 7 ms per
frame at scale 2 in a half-width pane.

Git snapshot (status + 2000 commits + refs) on the worker thread:

| Repo | Time |
|---|---|
| gitgui (4 commits) | 6 ms |
| vllm (20k commits, capped at 2000) | 660 ms |

The UI never waits for it; the first frame shows "loading" until the
snapshot arrives. Filesystem polling every 2 s only re-snapshots when a
watched mtime changed.

## Decisions

- Unstaging a hunk reverses the HEAD-to-index patch text (swap hunk ranges
  and +/- prefixes, keep the file headers) and applies it to the index.
- Network ops run on the git worker thread through the git CLI with
  `GIT_TERMINAL_PROMPT=0`; output streams into a log panel. A slow push blocks
  other git commands until it finishes, the UI stays responsive.
- `commit.gpgsign=true` routes commits through `git commit` so signing works.
- Manual testing on this machine: never inject mouse events while the user
  works; `cmux send` targets a specific surface and is safe, screenshots are
  only meaningful when the gitgui workspace is in front.

- git2 0.21 returns `Result` from most string getters (`shorthand`,
  `summary`, `Signature::name`, `StatusEntry::path`); unwrap to empty strings.
- Rename detection: the status entry's `path()` is the old path, the delta's
  `new_file()` has the new one.
- The graph keeps the first parent in the commit's lane; merge curves are
  drawn on the row where lanes join, forks on the merge commit's row.
- Log virtualization uses `ScrollArea::show_rows`; 2000 rows cost nothing
  off screen.

- Shm probe rides in the capability batch as `a=q,t=s`; both an `OK` reply and
  the object being unlinked are required (PROTOCOLS 4.2).
- DECRQM for mode 1016 decides between pixel and cell mouse coordinates,
  instead of guessing from coordinate ranges.
- Ctrl acts as egui's `command` modifier, because terminals never deliver Cmd.
- Ctrl+C always quits. `q` quits only when no text field has keyboard focus.
- Copy goes out as OSC 52. There is no read path back from the clipboard;
  paste arrives through bracketed paste.
- Frame pacing: the loop sleeps on the input channel until the repaint delay
  egui asked for. Idle CPU is zero unless a widget requests animation.

- Editor: the galley for the highlighted text is cached on a hash of the
  buffer, recomputed inside the layouter because `TextEdit` lays out again
  after each edit in the same frame. `Escape` that opens the unsaved-changes
  dialog is consumed so the dialog does not see it and close at once.
- Editor resolution for `Shift+E`: `--editor`, `git config gitgui.editor`,
  `$GITGUI_EDITOR`, `$VISUAL`, `$EDITOR`, `vi`. GUI editors run detached,
  matched on the basename of the first word; everything else gets a split.
- File tree: directories are listed by the worker on demand (`ListDir`),
  never walked eagerly; a snapshot re-lists the root and open folders. `.git`
  is skipped, ignored entries stay visible but dimmed.

## User requests (2026-09-03)

- UI must be usable at the terminal's own text size: font size now derives
  from the cell height (`cell_h / ppp * 0.76`), override with `--font-size`.
  Loading the terminal's font family itself (Ghostty `font-family`, resolved
  through CoreText / fontconfig) is a follow-up; egui's bundled fonts are
  used until then.
- A status bar at the bottom listing the available commands: present, and
  visible now that the scale bug is fixed.
- cmux panes resize and move between displays: SIGWINCH re-reads the grid
  from the ioctl, re-queries the cell size in-band (`CSI 16 t`), and when the
  cell height changes the scale and font size follow. Window resizes while a
  frame is in flight are coalesced by the identical-frame skip.

## Review 2026-09-04

Full pass over the code after v0.1.4 (three review agents plus headless
frames at 800x500, 550x350 and 1672x935 pt). Clippy and 108 tests green
before and after.

### Fixed in this pass

Layout (all visible in `--headless-frame` at 1600x1000 scale 2 before the fix):

- Footer text (path, branch, counts) overlapped the Quit and Refresh buttons
  in panes under about 900 pt. Cause: the trailing `right_to_left` layout
  was allocated after the leading labels and simply overflowed to the left.
  New `ui/row.rs` helper lays out the trailing widgets first and clips the
  leading side to what is left; used by the footer, the diff header and the
  commit button row. The footer drops the path first, then truncates the
  counts, so the branch switcher always stays visible.
- Commit box vanished below the footer in short detail panes. Cause: nested
  `allocate_ui_with_layout` grows past the requested height when the lists
  overflow, and `ScrollArea` has a 64 pt minimum. The lists and the commit
  box now get hard rects (`scope_builder` + `max_rect` + clip) and the
  scroll areas use `min_scrolled_height(0)`. During egui's panel sizing
  pass the detail pane reports a fixed 260 pt so the bottom panel does not
  balloon to the whole window.
- Commit summaries wrapped to two lines and clipped inside a single row.
  Now `layout_no_wrap` with the existing clip rect.
- Hunk header text ran underneath the `Stage hunk` button. Text is clipped
  to the space left of the button; in wrap mode the wrap width reserves the
  button too.
- Empty "nothing staged" list took half of the list column while the
  unstaged list showed one row. `list_scroll_heights` now takes row counts;
  a list only gets what its rows need, the rest goes to the other list.
- Toolbar wider than the footer in narrow panes (Quit wrapped off screen).
  Below 560 pt the fetch/pull/push/refresh buttons show icons only.
- Sidebar defaulted to 220 pt even in a 550 pt pane. Default is now 25 % of
  the width, clamped to 140..220.
- Amend checkbox label truncated to a stray dot in narrow columns; the label
  is dropped below 72 pt (hover text still explains it).

Robustness:

- Agent socket directory and socket file are now chmod 0700 / 0600. Before,
  another local user could drive the repository through `$TMPDIR/gitgui`.
- `--size WxH` rejects 0 and anything above 16384 instead of allocating a
  zero or multi-gigabyte framebuffer.
- Texture atlas partial update with an x offset past the atlas width no
  longer indexes out of bounds.
- Commit list row drawing uses `get` instead of indexing the commits vec, so
  a stale selection during a snapshot swap cannot panic.
- Filter changes kept `Selection::Commit(i)` on a commit the filter hid (or
  on the working tree row, which a filter hides); the next `j` / `k` jumped
  to the top. `rebuild_filter` now moves the selection to the first visible
  row when the current one disappears.

### Reviewed and left as is

- Bracketed paste ends at the literal `ESC [ 201 ~`; pasted text containing
  that sequence is cut short. Protocol limitation, no escaping exists.
- Unstaging in an empty repository ignores `remove_path` / `remove_dir`
  errors: exactly one of the two applies per path, the other always fails.
- Hunk staging skips binary deltas; the UI never offers hunk buttons for a
  binary diff, so nothing is silently lost.
- Untracked files above 2 MB are read once to decide `too_large`. Cheap
  enough, only happens when the file is selected.
- Screenshot path from the agent socket is not sandboxed. The socket is
  owner-only now, and the agent runs as the same user anyway.

### Open findings

- Detail pane under about 200 pt: the staged header gets clipped by the
  commit box. Acceptable, but hiding the staged section when it does not fit
  would look cleaner.
- Ghostty `font-family` is still not loaded; egui bundled fonts.
- `probe.rs` parses the reply buffer twice (once to check DA arrival, once
  for real). Harmless.
- Test gaps: paste containing ESC, kitty super modifier, malformed SGR
  mouse, escape split at odd byte boundaries, atlas update past bounds.

## Feature audit 2026-09-05

The feature set of the most used terminal git client was mapped against
gitgui, panel by panel. Everything below was missing and is now in. The
git layer gained `git/actions.rs` (git2) and `git/rebase.rs` (rebase todo
rewriting); the worker got `SetDiffOpts` and the CLI-backed sequencer
commands; the UI got a commit menu (`ui/menus.rs`), a help table
(`ui/help.rs`), generic confirm and input dialogs, and a diff viewer with
line selection and search.

Added:

- Working tree: line-level stage / unstage / discard (click, Shift+click,
  drag), discard hunk, discard all, ignore file, copy path, toggle staged
  with Space, stash with keep-index and include-untracked options, stash
  apply, branch from stash.
- Diff: search with highlighted matches and next / previous, context lines
  `{` / `}` (0 to 20), ignore whitespace. Hunk indices always come from a
  diff produced with the same options, so hunk staging stays consistent.
- Commits: new branch, tag (light or annotated), checkout detached,
  cherry-pick, revert, reset soft / mixed / hard, copy hash / message, open
  in browser (GitHub, GitLab, Bitbucket style URLs). History rewriting on
  the current branch: reword (amend for HEAD, rebase otherwise), squash,
  fixup, drop, move up / down, edit, create fixup commit, autosquash. All
  through `git rebase -i` with gitgui as the sequence editor; nothing
  interactive ever opens.
- Branches: rename, merge into current (fast-forward when possible, merge
  commit otherwise), rebase current onto, fast-forward from upstream, set /
  unset upstream, open pull request, delete on remote. Push sets the
  upstream automatically when the branch has none and an origin exists.
  Force push with lease and pull with rebase from the toolbar menus.
- Remotes: add, rename, edit URL, remove, fetch one. Tags: delete, push.
- State: merge, rebase (with progress), cherry-pick and revert show a
  footer banner with Continue / Abort, `m` adds Skip. Conflicted files show
  the file with its markers (ours as removed, theirs as added) and resolve
  with Use ours / Use theirs / Mark resolved from the file menu or the diff
  header.
- `?` help dialog generated from one table.

Left out on purpose (still available in the git CLI next door):

- Range selection in lists, file tree view, custom patches across commits,
  reflog undo / redo, bisect, worktrees, submodules, git-flow, editing
  files in an external editor (it would fight over the terminal),
  amending a non-HEAD commit with staged changes, reset author, moving
  commits to a new branch, commit filtering by path, screen modes.

Bugs found on the way:

- libgit2 caches the index per repository and does not re-read it before
  most operations. When another process (the test helper, the CLI, an
  agent) wrote the index, writes used stale entries. `Repo::index()` now
  re-reads it before every write; rule 11 in CLAUDE.md.
- libgit2's cherry-pick and revert leave their result in the in-memory
  index and their safe checkout neither creates new files nor deletes
  removed ones in this version. `finish_pick` writes the index, commits
  from it and syncs the touched paths from HEAD with a forced checkout
  limited to those paths.
- Stash apply in libgit2 refuses a dirty index, unlike the CLI. The error
  is shown as is.

## Next steps (resume here)

### Post v0.1 feature backlog

Priority order for the next releases. Sizes: small = an evening, medium = a
few sessions, large = a milestone.

1. **Terminal fonts** (medium): load Ghostty `font-family` via CoreText /
   fontconfig so UI text matches the terminal face at the computed size.
2. **File history and blame** (medium): `h` on a file opens its log,
   `b` opens blame in the diff pane. Read-only, `git2` has both.
3. **tmux / Zellij passthrough** (medium): detect and enable kitty graphics
   passthrough instead of hard exit.
4. **Repo switcher** (small): recent repos list, `--repo` history file
   under XDG config, `Ctrl+R` picker; needs a worker `OpenRepo` command.
5. **Multi-select** (medium): Shift+click ranges in the file lists, stage /
   unstage / discard the range; file tree view with collapsing directories.
6. **Reflog** (medium): read-only reflog list in the sidebar, undo / redo of
   the last ref move.
7. **Agent API parity** (small): expose checkout, branch, tag, cherry-pick,
   revert, reset and state actions on the socket.
8. **Per-hunk ours / theirs** (medium): resolve one conflict block at a time
   in the conflict view instead of the whole file.

Done 2026-09-04: README demo (inline GIFs, 960 px, 6 fps). Done 2026-09-05:
diff search, conflict UI, commit menu, remote management (see "Feature
audit 2026-09-05").

Not planned for v0.x: an interactive rebase editor, bisect, submodules,
worktrees, sparse checkout, patch export. Use the git CLI in the
neighbouring pane.

### Recently shipped (v0.1.1 to v0.2.0)

- v0.2.0: feature parity pass, see "Feature audit 2026-09-05"

- v0.1.6: commit detail shows the full message body, word-wrapped, above
  the file list (`CommitRow::body`); footer ahead/behind uses words because
  the bundled fonts have no arrow glyphs

- v0.1.5: review pass, see "Review 2026-09-04"
- v0.1.4: footer toolbar with fetch / pull / push / refresh / quit, branch
  picker with dirty-tree confirm, publish to GitHub via `gh`, init screen
  for non-git folders, compact commit box, wrap-aware diff row heights
- v0.1.3 and earlier:

- Install script fix (binary vs tar.gz)
- Commit and push button and worker command
- Status bar key letter highlighting
- Keyboard: `Shift+S` stash, `Ctrl+Shift+Enter` commit and push, `r` refresh (manual)
- Agent API: `fetch`, `pull`, `push`, `commit_and_push`
- **Auto refresh**: git worker polls every 2 s; refreshes when worktree fingerprint or `.git/` mtimes change, even when the gitgui pane is unfocused (edits from pi in a neighboring cmux pane show up without pressing `r`)

### Editor and tree follow-ups

- Keyboard navigation inside the file tree (j / k, Enter, Left / Right).
- Editor: search (`Ctrl+F` currently belongs to the diff), go to line,
  soft wrap toggle, tab width from `.editorconfig`.
- Highlighter: string interpolation, doc comments, more languages on demand.

### Manual verification still owed

- File tree clicks, folder expansion and the file menu in a real pane
  (2026-09-05: tree rendering and `e` verified on screen, clicks only in the
  harness test).

Commit via button, Stage hunk, branch context menus, fetch/pull/push log,
branch picker, publish to GitHub. The 2026-09-04 layout fixes were verified
with headless frames at three sizes; a real cmux pane check of the narrow
(icon-only) footer and the short detail pane is still owed. Harness tests
cover the commit button; layout math is unit-tested in `changes.rs`.

2026-09-05 additions verified headless (1600x1000, 1100x700, 3344x1870 at
scale 2, and a merge-conflict fixture at 1200x760) and through harness
tests for the keys (`?`, `{` `}`, `Ctrl+W`, `Ctrl+F`, `s` on a line
selection, `Shift+D`, `Shift+T`, `g`, `Shift+R`, `d`, `n`, `y`). The git
layer has tests for every new operation including a real `git rebase` and
`--abort` through the worker. Still owed in a real pane: right-click menus
(egui context menus need a real secondary click), drag selection in the
diff, the rebase editor round trip (`gitgui --sequence-editor` needs the
installed binary, the test binary cannot play that role), and the footer
banner during a rebase.

## Open issues

- SSH manual check not done: Remote Login is off on the dev Mac. The direct
  transport was verified with `--no-shm` instead.
- `cmux send-key` does not deliver keys to the pane. `cmux send` types text
  through the terminal's key encoder (ESC arrives as `CSI 27 u`), so it can
  exercise the keyboard path but not the mouse path. Mouse checks use
  `scripts/click.swift`, which posts real CoreGraphics events.
- Terminal cursor shape cannot follow egui's cursor icon. Ignored.
- The demo app's spinner is off by default so an idle session uses no CPU.
  Turning it on repaints at 60 fps on purpose.

## Closed issues

- Detail column overlapped the commit box in short panes (~225 pt): nested
  `Panel::bottom` did not clip the lists above it. Fixed with explicit
  height allocation (`allocate_ui_with_layout`) and shrinking commit rows.

- A gitgui whose pane was closed kept running at 100% CPU: the stdin thread
  treated EOF like a timeout and spun, and the main loop could sleep up to an
  hour before looking at the SIGHUP flag. EOF now ends the thread (the loop
  exits on the closed channel) and the idle wait is capped at 500 ms.

- Phase 3/4: on screen the UI was drawn at 4x (2x too big), clicks landed on
  the wrong widgets and the status bar was off screen. Cause: egui's
  `pixels_per_point = zoom_factor * native_pixels_per_point`; the runtime set
  the zoom via `set_pixels_per_point` AND passed the native scale. Only the
  native scale is passed now, with a regression test.

- Phase 1: glyph rendering cost 17 ms of a 20 ms full-pane frame. Fixed with
  transparent texel skipping and nearest sampling for pixel-snapped glyphs.
- Phase 1: framebuffer `mark_sent` copied 12 MB per frame. Now swaps buffers.
