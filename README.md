# gitgui

A git GUI that runs inside your terminal.

Not a TUI. gitgui renders a real pixel interface (commit graph, staging area, diff viewer) into your existing terminal pane using the kitty graphics protocol. One small Rust binary, no browser engine, no Electron, works over SSH.

Status: early development. Phases 0 to 2 of the roadmap are done (terminal plumbing, rendering, input). See [Roadmap](#roadmap) and [docs/PLAN.md](docs/PLAN.md) for the live plan and open issues.

## Install (macOS & Linux)

From source, until release binaries exist:

```
git clone https://github.com/antonellof/gitgui
cd gitgui
cargo install --path .
```

Requires a Rust stable toolchain (1.95 or newer) and a terminal that speaks the kitty graphics protocol: Ghostty, cmux, kitty, WezTerm.

## Usage

```
gitgui                    open the repository containing the current directory
gitgui <path>             open the repository at <path>
gitgui --probe            print what the terminal supports and exit
gitgui --dump-input       print decoded key and mouse events, Ctrl+C to exit
gitgui --no-shm           force the base64 transport (what SSH uses)
gitgui --scale 2          override pixels per point (auto-detected from the cell height)
gitgui --font-size 14     UI font size in points
gitgui --help
```

Inside gitgui, press `q` or `Ctrl+C` to quit.

Not every flag listed in the spec exists yet. Run `gitgui --help` for the current set.

## How does it work?

The terminal never sees any text. gitgui paints an [egui](https://github.com/emilk/egui) interface into an RGBA framebuffer with its own software rasterizer and ships every frame to the terminal as a kitty graphics image covering the whole pane. Locally the frame goes through POSIX shared memory, so a 1600x1000 frame costs one page-table update. Over SSH it falls back to zlib plus base64 inline.

Input comes back the same way a terminal application receives it: the kitty keyboard protocol for keys, SGR pixel mouse for clicks, drags and wheel, bracketed paste, focus events, SIGWINCH for resize. The bytes are decoded into egui events, so every widget behaves like it does in a native window.

Git access goes through libgit2 for reads and index writes. Network operations (fetch, pull, push) shell out to the `git` CLI so your credential helpers and SSH agent keep working unchanged.

Same idea as [terminal-browser](https://github.com/zenbu-labs/terminal-browser) and [terminal-code](https://github.com/zenbu-labs/terminal-code), minus Chromium.

## SSH

Run `gitgui` on the remote machine inside an SSH session in a supported terminal. It detects `SSH_TTY`, switches to the inline transport and throttles to 20 fps. Nothing needs to be installed on the local side.

## Shortcuts

| Action | Key |
|---|---|
| Move selection | `j` / `k`, `Down` / `Up` |
| Open, checkout | `Enter` |
| Stage, unstage | `s` / `u` |
| Stage all, unstage all | `a` / `Shift+A` |
| Focus commit message | `c` |
| Commit | `Ctrl+Enter` |
| Filter commits | `/` |
| Clear filter, close modal | `Escape` |
| Fetch, pull, push | `f` / `p` / `Shift+P` |
| Refresh | `r` |
| Cycle panes | `Tab` |
| Quit | `q`, `Ctrl+C` |

gitgui never binds `Cmd+*` or `Ctrl+Shift+*`; those stay with your terminal and multiplexer.

## Roadmap

- [x] Phase 0: terminal plumbing. Capability probe, raw mode, kitty graphics frame transport with shm and inline paths, clean restore on quit, panic and signals.
- [x] Phase 1: rendering. Software rasterizer for egui meshes, headless PNG frames.
- [x] Phase 2: input. Kitty keyboard, SGR pixel mouse, paste, focus, resize.
- [ ] Phase 3: read-only git. Sidebar, commit graph, commit detail, diff view.
- [ ] Phase 4: writes. Stage and unstage by file and hunk, commit, amend, branches, stash, fetch, pull, push.
- [ ] Phase 5: integration. Split panes in cmux and Ghostty, agent control socket, install script, release builds.

## Development

```
cargo test                          unit tests, byte-exact for every protocol encoder and parser
cargo clippy -- -D warnings
cargo run -- --probe                what does this terminal support?
cargo run --release -- --headless-frame /tmp/frame.png --size 1600x1000 --scale 2
                                    render one frame to a PNG without a terminal, prints timings
cargo run --release                 interactive
```

The design documents are the source of truth: [docs/SPEC.md](docs/SPEC.md) for architecture and milestones, [docs/PROTOCOLS.md](docs/PROTOCOLS.md) for the exact escape sequences.

tmux and Zellij are not supported yet (kitty graphics need passthrough). gitgui detects them and exits with a message.

## License

MIT
