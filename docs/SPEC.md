# gitgui: specification

## 1. Goal

A git GUI comparable in feel to Sourcetree or GitKraken's core panels (graph, changes, diff, stage by hunk, commit, branch switching), rendered as pixels inside an existing terminal pane. Instant startup, a few MB binary, works over SSH, zero browser engine.

Target terminals: Ghostty and cmux first, kitty second. Anything with kitty graphics + SGR pixel mouse should work.

Non-goals for v1: merge conflict resolution UI, interactive rebase editor, submodules, worktrees, Windows.

## 2. Architecture

```
                  stdin bytes                 Event channel
  Terminal  ─────────────────►  term::input  ───────────────►  main loop
     ▲                                                             │
     │  APC G frames (shm or base64)                               │ egui RawInput
     │                                                             ▼
  term::kitty  ◄──────  render::frame  ◄──────  render::raster  ◄──── egui::Context::run(app::ui)
                         (RGBA, dirty check)     (meshes -> pixels)        │
                                                                           │ reads RepoSnapshot, emits Command
                                                                           ▼
                                                              git worker thread (git2)
                                                              produces new RepoSnapshot
```

Threads:

- **input thread**: blocking `read(2)` on stdin, pushes raw bytes to a channel. The main loop drains the channel, feeds `term::input::Parser`, gets `Vec<Event>`.
- **main loop**: waits on input channel or a repaint deadline, builds `egui::RawInput`, runs the UI, tessellates, rasterizes, sends the frame if changed.
- **git worker**: receives `Command`, executes with `git2` or the `git` CLI, sends back `RepoSnapshot` or `OpResult`. The UI never blocks on git.

Frame pacing: render only when egui requests a repaint (`FullOutput::viewport_output` repaint delay) or an event arrived. Idle CPU must be zero.

### 2.1 Software rasterizer (render/raster.rs)

Input: `Vec<egui::ClippedPrimitive>` from `ctx.tessellate(shapes, pixels_per_point)` plus `TexturesDelta`.

- Maintain a texture map `TextureId -> Texture { w, h, rgba: Vec<[u8;4]> }`. Apply `TexturesDelta::set` (full or partial via `ImageDelta::pos`) before drawing, `TexturesDelta::free` after. `ImageData::Font` becomes RGBA via `FontImage::srgba_pixels(None)`; `ImageData::Color` is already RGBA.
- For each `Primitive::Mesh`, iterate triangles (`indices` in groups of 3). Compute the bounding box, intersect with `clip_rect` scaled by `pixels_per_point` and the framebuffer bounds. For each pixel center in the box, compute barycentric weights with edge functions; skip if outside. Interpolate `uv` and `color`, sample the texture with bilinear filtering (nearest is acceptable for phase 1, bilinear needed for readable text at scale 1.0), multiply, and blend onto the framebuffer.
- egui vertex colors are premultiplied alpha in gamma space. Blend: `dst = src + dst * (255 - src_a) / 255` per channel, no gamma conversion. Text looks right with this.
- Top-left fill rule to avoid double-blending shared edges.
- `Primitive::Callback` is ignored.
- Performance targets: 1600x1000 frame with a full commit list under 8 ms in release. Process triangles in scanline order, precompute edge function increments, and avoid per-pixel `f32 -> usize` casts in the inner loop where possible. Use `rayon` only if the target is missed; prefer not to.

### 2.2 Framebuffer (render/frame.rs)

Two `Vec<u8>` RGBA buffers of `w*h*4`. Clear with the theme background each frame. After rasterizing, compare against the last sent buffer (memcmp, it is fast); send only if different. Export to PNG for `--headless-frame`.

### 2.3 Input mapping to egui

- `KeyDown/KeyUp` -> `egui::Event::Key { key, physical_key: None, pressed, repeat, modifiers }`. Map codepoints to `egui::Key` where one exists; unmapped keys still produce `Event::Text` when they carry text.
- Text -> `egui::Event::Text(String)`, but never for ctrl/alt combos and never for Enter, Tab, Escape, Backspace.
- Mouse press/release -> `Event::PointerButton { pos, button, pressed, modifiers }`, motion -> `Event::PointerMoved(pos)`, wheel -> `Event::MouseWheel { unit: Point, delta, modifiers }` with coalescing (see PROTOCOLS 3.2). Positions are `pixel / pixels_per_point`.
- Focus lost -> `Event::WindowFocused(false)`, and release all buttons.
- Resize -> new `screen_rect` in `RawInput`, framebuffer reallocation.
- `RawInput.time` from a monotonic clock, `max_texture_side` 8192, `predicted_dt` 1/60.

Also provide `egui::Event::Copy/Cut/Paste`: copy writes to the terminal clipboard with `OSC 52 ; c ; <base64> ST` (Ghostty and kitty support it, subject to the terminal's clipboard permission setting). Paste comes from bracketed paste.

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
Refresh
Stage(paths) | Unstage(paths) | StageAll | UnstageAll | Discard(paths)      // Discard asks confirmation in UI
StageHunk { path, hunk_index } | UnstageHunk { path, hunk_index }
Commit { message, amend: bool }
Checkout(branch) | CreateBranch { name, from } | DeleteBranch(name)
StashPush { message } | StashPop(index) | StashDrop(index)
Fetch | Pull | Push                                                        // git CLI, output streamed to a log panel
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

Layout, egui:

```
┌ sidebar 220pt ┬ main ─────────────────────────────────────────────┐
│ Local         │ commit list (graph | refs + summary | author | age)│
│  * main       │  row 0 is the virtual "Working tree" row when dirty │
│    feature/x  ├────────────────────────────────────────────────────┤
│ Remote        │ detail pane (resizable, default 45%)               │
│  origin/main  │  commit selected: files list | diff                │
│ Tags          │  working tree selected: unstaged | staged | diff   │
│ Stashes       │                          commit message + button   │
└───────────────┴────────────────────────────────────────────────────┘
status bar: repo path, branch, ahead/behind, last op result, key hints
```

Behaviors:

- Click a branch: select its tip in the list. Double click or `Enter`: checkout. Right click: context menu (checkout, new branch from here, delete, copy name).
- Click a commit: load files and diff for the first file. Refs render as colored pills before the summary.
- Working tree row: unstaged and staged lists side by side; click a file to show its diff; click the `+` / `-` icon or press `s` / `u` to stage or unstage; `Stage all` / `Unstage all` buttons; `Discard` with a confirmation modal.
- Diff view: monospace, line numbers for old and new, colored backgrounds for + and - lines, hunk headers with a `Stage hunk` / `Unstage hunk` button, horizontal scroll, word-wrap toggle. Syntax highlighting is out of scope for v1.
- Commit box: multiline text edit, `Ctrl+Enter` commits, amend checkbox, shows the author from config.
- Search: `/` focuses a filter box over the commit list (summary, author, short hash).
- Toast notifications for op results, errors in red with the git stderr text.
- Everything must be operable with mouse only and with keyboard only.

### Keybindings

Single keys and Ctrl combos the terminal does not claim:

```
j / k, Down / Up      move selection          Enter          open / checkout
s / u                 stage / unstage         a / Shift+A    stage all / unstage all
c                     focus commit message    Ctrl+Enter     commit
/                     filter                  Escape         clear filter / close modal
f / p / Shift+P       fetch / pull / push     r              refresh
Tab                   cycle focus between sidebar, list, detail
q, Ctrl+C             quit
```

### Theme

Dark default. Query the terminal background (PROTOCOLS section 5); if it is light, use the light theme. Fonts: egui's bundled fonts at 13 pt UI, 12.5 pt monospace for diffs. Allow `--font-size` override.

## 5. CLI

```
gitgui [path]                     open repo at path (default: discover from cwd)
  --split right|left|down|up            open in a new terminal split (see section 6)
  --size 0.2..0.95                      fraction of the pane the split takes
  --scale 1|1.5|2                       override pixels_per_point
  --font-size N
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
version = "0.1.0"
edition = "2021"

[dependencies]
egui = { version = "*", default-features = false, features = ["default_fonts"] }   # pin to latest stable at start
git2 = { version = "*", default-features = false }                                  # add "vendored-libgit2" for release builds
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

Pin exact versions on day one and record the egui API version in CLAUDE.md, since `egui::Context::run`, `tessellate`, and `TexturesDelta` have shifted slightly between releases.
