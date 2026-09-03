# gitgui

A pixel-rendered git GUI that runs inside kitty-graphics terminals (Ghostty, cmux, kitty). No Chromium, no Electron. A single Rust binary renders an egui UI into an RGBA framebuffer, ships frames to the terminal with the kitty graphics protocol, and reads pixel-precise mouse and keyboard input back from the terminal.

Reference projects for the idea (not the implementation): zenbu-labs/terminal-browser and zenbu-labs/terminal-code. We reuse their trick (pixels in the terminal via kitty graphics + synthetic input) but skip the browser engine entirely.

Read `docs/SPEC.md` (architecture, UI, git layer, milestones) and `docs/PROTOCOLS.md` (exact escape sequences) before writing code. They are the source of truth. If the spec and this file disagree, the spec wins.

## Stack

- Rust, latest stable, edition 2021
- `egui` + `epaint` for the UI model and tessellation (we do NOT use eframe or any GPU backend)
- Our own software rasterizer for egui meshes (`src/render/raster.rs`)
- `git2` (libgit2) for all repository reads and index/commit writes; `git` CLI subprocess only for network ops (fetch, pull, push)
- `libc` for termios, ioctl, POSIX shared memory
- `flate2` + `base64` for the SSH fallback transport
- `png` crate for headless frame dumps used in tests

No async runtime. One thread for the UI loop, one thread that reads stdin into a channel, one worker thread for slow git operations.

## Module map

```
src/
  main.rs            CLI parsing, mode dispatch (interactive, headless-frame, dump-input)
  term/
    mod.rs           raw mode, alt screen, enable/disable sequences, restore on exit and panic
    probe.rs         capability probing: kitty graphics, kitty keyboard, cell size, pixel size
    input.rs         byte stream -> Event (keys, mouse in pixels, resize, focus, paste)
    kitty.rs         kitty graphics encoder: shm transport, direct transport, place, delete
  render/
    raster.rs        triangle rasterizer for epaint meshes, texture atlas, clip rects
    frame.rs         double-buffered RGBA framebuffer, dirty detection, headless PNG export
  git/
    repo.rs          Repository wrapper: status, branches, log, diffs, stage/unstage, commit
    graph.rs         commit graph lane assignment
    ops.rs           slow ops on the worker thread (fetch/pull/push via git CLI)
  ui/
    app.rs           top-level egui app state and layout
    sidebar.rs       branches, remotes, tags, stashes
    log.rs           commit list with graph column
    changes.rs       working tree: unstaged/staged file lists, commit box
    diff.rs          diff viewer with per-hunk stage/unstage
    theme.rs         colors derived from terminal palette (OSC 10/11 query, fallback dark)
  split.rs           open in a terminal split (cmux, Ghostty) with in-place fallback
  agent.rs           unix socket JSON-lines control API (phase 5)
```

## Pinned versions

Toolchain at kickoff: rustc 1.98.0, cargo 1.98.0 (2026-08). Both crates below are pinned with `=` in Cargo.toml.

- `egui = "=0.36.1"`, `epaint = "=0.36.1"` (rust-version 1.95). API notes:
  - `egui::Context::run` no longer exists. Use `ctx.run_ui(raw_input, |ui| ...)` which returns `FullOutput`, or `begin_pass` / `end_pass`.
  - `FullOutput` fields: `platform_output`, `textures_delta`, `shapes: Vec<ClippedShape>`, `pixels_per_point`, `viewport_output`.
  - `ctx.tessellate(shapes, pixels_per_point) -> Vec<ClippedPrimitive>`.
  - `epaint::ImageData` has a single variant `Color(Arc<ColorImage>)`. There is no `Font` variant; the font atlas arrives as premultiplied RGBA `ColorImage` (`pixels: Vec<Color32>`, `as_raw()` for bytes). `ImageDelta { image, options, pos: Option<[usize; 2]> }`.
  - `TexturesDelta` lives at `epaint::textures::TexturesDelta`. `set` is `HashMap<TextureId, SmallVec<[ImageDelta; 1]>>` (apply each delta in order), `free` is `HashSet<TextureId>`.
  - Dropping a `TexturesDelta` without applying it panics in debug builds, even after you applied it by reference: call `clear()` once done.
  - Panels: `SidePanel` and `TopBottomPanel` are gone. Use `egui::Panel::left("id").default_size(220.0).show(ui, ...)`, `Panel::bottom(...)`, and `CentralPanel::default().show(ui, ...)`. All take the root `&mut Ui` that `run_ui` hands to the closure, not a `Context`.
- `git2 = "=0.21.0"` with `default-features = false` (builds libgit2 from source, no system dependency). Most string getters return `Result` in this version (`Reference::shorthand`, `Commit::summary` gives `Result<Option<&str>>`, `Signature::name`, `StatusEntry::path`, `StringArray::iter` yields `Result<Option<&str>>`).
- `png = "0.17"`, used by `--headless-frame`.

## Commands

```
cargo build --release
cargo test
cargo run -- --headless-frame /tmp/frame.png --repo .      # render one frame to PNG, no terminal needed
cargo run -- --dump-input                                   # print parsed input events, Ctrl+C to exit
cargo run -- --probe                                        # print detected terminal capabilities
cargo run --release                                         # interactive, in current repo
```

## Working rules

1. Every module that parses or encodes bytes gets unit tests with literal byte sequences. `term/input.rs` and `term/kitty.rs` must have snapshot-style tests. Do not skip them, they are the only cheap safety net for protocol code.
2. You cannot see the kitty graphics output from inside a test. Use `--headless-frame` to produce a PNG and inspect it with the image viewer tool to verify rendering. Do this at the end of every phase.
3. Terminal state must be restored on every exit path: normal quit, error, panic, SIGINT, SIGTERM. Install a panic hook that restores the terminal before printing the panic. Test by inserting a deliberate `panic!()` once and confirming the shell is usable afterwards.
4. Never consume terminal or multiplexer shortcuts. Ghostty and cmux own `Cmd+*` on macOS and most `Ctrl+Shift+*`. Our bindings are single keys and `Ctrl+` letters that terminals do not claim (see SPEC, "Keybindings").
5. Full-frame updates only. Do not attempt partial image updates via kitty animation frames (`a=f`); Ghostty support is not guaranteed. Skip the send when the frame is byte-identical to the last one sent.
6. Locally use the shared memory transport. Fall back to direct base64+zlib when `SSH_TTY` or `SSH_CONNECTION` is set or when the shm probe fails.
7. Git writes go through `git2`. Network goes through the `git` CLI so credential helpers and SSH agents work unchanged. Never implement credential handling yourself.
8. Keep the UI loop under 16 ms for a 1600x1000 frame on an M-series or recent x86 laptop. Profile the rasterizer before optimizing anything else; it is the only hot path.
9. Do not add dependencies beyond the stack above without stating why in the commit message. In particular no `crossterm`, no `ratatui`, no `tokio`.
10. Do not use the em dash character anywhere in code, comments, docs, or commit messages. Use a comma, a colon, or a period.

## Conventions

- `anyhow::Result` at boundaries, typed errors inside `git/` and `term/`
- `Event` and `Command` enums, no callbacks across modules
- Rendering never touches git. UI reads from an immutable `RepoSnapshot` that the git worker replaces atomically after each operation.
- Keep `main.rs` under 200 lines. It wires modules, nothing else.
- Commit after every milestone step with a message that names the step (see SPEC, "Milestones").

## Definition of done per phase

A phase is done when: `cargo test` is green, `cargo clippy -- -D warnings` is clean, the headless PNG for that phase looks right, and the manual check listed in the SPEC for that phase has been performed in a real Ghostty or cmux pane.
