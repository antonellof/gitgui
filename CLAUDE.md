# gitgui

A pixel-rendered git GUI that runs inside kitty-graphics terminals (Ghostty, cmux, kitty). No Chromium, no Electron. A single Rust binary renders an iced UI with the tiny-skia software renderer into an RGBA framebuffer, ships frames to the terminal with the kitty graphics protocol, and reads pixel-precise mouse and keyboard input back from the terminal. Without kitty graphics (or with `--window`) the same UI opens in a native window (`window.rs`).

Reference projects for the idea (not the implementation): zenbu-labs/terminal-browser and zenbu-labs/terminal-code. We reuse their trick (pixels in the terminal via kitty graphics + synthetic input) but skip the browser engine entirely.

Read `docs/SPEC.md` (architecture, UI, git layer, milestones) and `docs/PROTOCOLS.md` (exact escape sequences) before writing code. They are the source of truth. If the spec and this file disagree, the spec wins.


## Stack

- Rust, latest stable, edition 2021
- `iced_core` + `iced_runtime` + `iced_widget` + `iced_renderer` (tiny-skia backend only, no wgpu) for the terminal path
- `iced` umbrella crate (tiny-skia + winit + softbuffer, no wgpu) only for `window.rs`, the desktop window fallback
- `tiny-skia` for the pixmap the renderer draws into
- `git2` (libgit2) for all repository reads and index/commit writes; `git` CLI subprocess only for network ops (fetch, pull, push)
- `libc` for termios, ioctl, POSIX shared memory
- `flate2` + `base64` for the SSH fallback transport
- `png` crate for headless frame dumps used in tests

No async runtime. One thread for the UI loop, one thread that reads stdin into a channel, one worker thread for slow git operations.

## Module map

```
src/
  main.rs            wires modules, mode dispatch (interactive, headless-frame, dump-input, probe)
  cli.rs             argument parsing and --help
  runtime.rs         interactive main loop: input channel, shell frame, frame send, resize
  shell.rs           iced without a window: events -> iced events, UserInterface build/update/draw, tiny-skia into the framebuffer
  term/
    mod.rs           raw mode, alt screen, enable/disable sequences, restore on exit and panic
    probe.rs         capability probing: kitty graphics, kitty keyboard, cell size, pixel size
    input.rs         byte stream -> Event (keys, mouse in pixels, resize, focus, paste)
    kitty.rs         kitty graphics encoder: shm transport, direct transport, place, delete
  render/
    frame.rs         double-buffered RGBA framebuffer, dirty detection, headless PNG export
  git/
    repo.rs          Repository wrapper: status, branches, log, diffs, stage/unstage, commit
    actions.rs       cherry-pick, revert, merge, reset, tags, remotes, upstreams, conflicts, line-level patches
    ai.rs            commit message suggestions: prompt from the staged patch, piped into an external AI CLI (claude, codex, gemini, ollama, llm or gitgui.ai-command)
    rebase.rs        rebase todo rewriting; gitgui is its own GIT_SEQUENCE_EDITOR / GIT_EDITOR
    graph.rs         commit graph lane assignment
    ops.rs           worker thread: Command enum, git2 writes, git CLI for network and rebase
  ui/
    app.rs           App state, Message enum, update, key bindings, pane_grid view
    sidebar.rs       collapsible sections: branches, remotes, tags, stashes, file tree
    tree.rs          sidebar file tree of the whole working tree, lazy listings via Command::ListDir
    log.rs           commit list: custom widget, graph via iced geometry, text helpers (draw_text, fit)
    changes.rs       conflicts, unstaged, staged lists, commit box, commit detail
    diff.rs          diff viewer: custom widget, hunk buttons, line selection, search, conflict banner
    merge.rs         three-way conflict resolver: marker parser, result builder, custom widget
    editor.rs        built-in file editor on iced text_editor, save, $EDITOR / cmux hand-off
    highlight.rs     dependency-free syntax highlighter, iced Highlighter impl for the editor
    footer.rs        footer (name, branch switcher, counts, merge banner, buttons) and the network log
    modal.rs         dialogs as a stack overlay
    menu.rs          right-click menus as a stack overlay, clamped into the window
    help.rs          keyboard reference table and the `?` dialog
    widgets.rs       buttons, rows, sections, pane chrome, toasts, Layered, widget ids
    undo.rs          undo / redo histories for text_editor contents and text_input values (iced has none)
    vsplit.rs        vertical splitter: draggable bars between the changes pane sections
    logo.rs          assets/logo.png compiled in: image handle for the no-repository screen, RGBA for the window icon
    state.rs         per-repository UI state (layouts, hidden panes, sections, wrap, columns) in <gitdir>/gitgui.json
    theme.rs         colors derived from terminal palette (OSC 10/11 query, fallback dark), iced Theme
  macos.rs           macOS dock icon for the bare binary (NSApplication setApplicationIconImage: over raw objc_msgSend)
  window.rs          desktop window mode: the same App through iced::application (winit + softbuffer), --window or no kitty graphics
  split.rs           open in a terminal split (cmux, Ghostty) with in-place fallback
  agent.rs           unix socket JSON-lines control API (phase 5)
```

## Pinned versions

Toolchain at kickoff: rustc 1.98.0, cargo 1.98.0 (2026-08). Both crates below are pinned with `=` in Cargo.toml.

- `iced_core`, `iced_runtime`, `iced_renderer` `=0.14.0`, `iced_widget` `=0.14.2`, `tiny-skia` `=0.11.4`. API notes:
  - Terminal path, no window: `iced_runtime::user_interface::UserInterface::build(element, size, cache, &mut renderer)`, `update(&events, cursor, &mut renderer, &mut clipboard, &mut messages)`, `draw(...)`, `into_cache()`. `iced_renderer::Renderer::new(font, size)` with only the tiny-skia feature; `Renderer::draw(&mut PixmapMut, &mut Mask, &Viewport, &[damage], bg)` rasterizes.
  - Custom widgets implement `iced_core::Widget`: `size`, `layout`, `update(tree, event, layout, cursor, renderer, clipboard, shell, viewport)`, `draw`. Widget state lives in `tree.state` (`tree::Tag::of::<State>()`).
  - Text in custom widgets goes through `log::draw_text` (handles tiny-skia's clip quirks) and `log::measure` / `log::fit` (a `Paragraph` per call: cache the results).
  - Geometry (the graph) uses `iced_widget::canvas::{Frame, Path, Stroke}` with `Frame::with_bounds` in absolute coordinates, never `with_translation`.
  - `text_editor` and `text_input` key bindings run even when unfocused: check `KeyPress::status`. Focus is moved with `iced_core::widget::operation::focusable::{focus, unfocus}` queued in `App::ops`.
  - Overlays go through `widgets::layered` so they draw above the custom widgets' layers.
- `git2 = "=0.21.0"` with `default-features = false` (builds libgit2 from source, no system dependency) plus `unstable-sha256` through our default `sha256` feature: libgit2 with GIT_EXPERIMENTAL_SHA256, so SHA-256 repositories open and commit (`Oid` holds up to 32 bytes; never assume 20). Most string getters return `Result` in this version (`Reference::shorthand`, `Commit::summary` gives `Result<Option<&str>>`, `Signature::name`, `StatusEntry::path`, `StringArray::iter` yields `Result<Option<&str>>`).
- `png = "0.17"`, used by `--headless-frame`.
- `GITGUI_HEADLESS_OPEN=picker|help|menu|stash|reset|merge|folder|update|hidden|zoom|select|difftext|detail` makes `--headless-frame` open that dialog or tool first, for visual review.

## Commands

```
cargo build --release
cargo test
cargo run -- --headless-frame /tmp/frame.png --repo .      # render one frame to PNG, no terminal needed
cargo run -- --dump-input                                   # print parsed input events, Ctrl+C to exit
cargo run -- --probe                                        # print detected terminal capabilities
cargo run --release                                         # interactive, in current repo
cargo run --release -- --window                             # desktop window (also automatic without kitty graphics)
```

## Working rules

1. Every module that parses or encodes bytes gets unit tests with literal byte sequences. `term/input.rs` and `term/kitty.rs` must have snapshot-style tests. Do not skip them, they are the only cheap safety net for protocol code.
2. You cannot see the kitty graphics output from inside a test. Use `--headless-frame` to produce a PNG and inspect it with the image viewer tool to verify rendering. Do this at the end of every phase.
3. Terminal state must be restored on every exit path: normal quit, error, panic, SIGINT, SIGTERM. Install a panic hook that restores the terminal before printing the panic. Test by inserting a deliberate `panic!()` once and confirming the shell is usable afterwards.
4. Never consume terminal or multiplexer shortcuts. Ghostty and cmux own `Cmd+*` on macOS and most `Ctrl+Shift+*`. Our bindings are single keys and `Ctrl+` letters that terminals do not claim (see SPEC, "Keybindings").
5. Full-frame updates only. Do not attempt partial image updates via kitty animation frames (`a=f`); Ghostty support is not guaranteed. Skip the send when the frame is byte-identical to the last one sent.
6. Locally use the shared memory transport. Fall back to direct base64+zlib when `SSH_TTY` or `SSH_CONNECTION` is set or when the shm probe fails.
7. Git writes go through `git2`. Network goes through the `git` CLI so credential helpers and SSH agents work unchanged. Never implement credential handling yourself. The one other CLI exception is the sequencer: rebase, and continue / abort / skip of an in-progress merge, rebase, cherry-pick or revert, because libgit2 has no equivalent. History rewrites run `git rebase -i` with gitgui as the sequence editor (`git/rebase.rs`), never an interactive editor.
8. Keep the UI loop under 16 ms for a 1600x1000 frame on an M-series or recent x86 laptop. Text layers and per-frame measuring are the hot paths; the custom widgets must draw only visible rows.
9. Do not add dependencies beyond the stack above without stating why in the commit message. In particular no `crossterm`, no `ratatui`, no `tokio`.
10. Do not use the em dash character anywhere in code, comments, docs, or commit messages. Use a comma, a colon, or a period.
11. Refresh the cached index (`Repo::index()`) before any write. libgit2 caches the index per repository and another process (the git CLI, an editor, the agent next door) may have written it since.
12. Never add Co-Authored-By, Claude-Session, or Generated with Claude Code lines to commit messages or PR bodies.

## Conventions

- `anyhow::Result` at boundaries, typed errors inside `git/` and `term/`
- `Event` and `Command` enums, no callbacks across modules
- Rendering never touches git. UI reads from an immutable `RepoSnapshot` that the git worker replaces atomically after each operation.
- Keep `main.rs` under 200 lines. It wires modules, nothing else.
- Commit after every milestone step with a message that names the step (see SPEC, "Milestones").

## Definition of done per phase

A phase is done when: `cargo test` is green, `cargo clippy -- -D warnings` is clean, the headless PNG for that phase looks right, and the manual check listed in the SPEC for that phase has been performed in a real Ghostty or cmux pane.

