# gitgui

A git GUI that runs inside your terminal, next to your coding agent. One Rust binary paints an [iced](https://iced.rs) interface as pixels into a cmux, Ghostty, kitty or WezTerm pane over the kitty graphics protocol. No Electron, no browser engine, no TUI. Works over SSH.


Install:

```bash
curl -fsSL https://raw.githubusercontent.com/antonellof/gitgui/main/scripts/install.sh | bash
```

Run:

```bash
cd /path/to/repo && gitgui
```

![gitgui in a cmux split next to Claude Code: repository, commit graph, changes and diff](screenshot/gitgui-cmux-claude.png)

<p align="center">
  <a href="screenshot/gitgui-commits.png"><img src="screenshot/gitgui-commits.png" width="100%" alt="Repository, commit graph, changes and diff on draggable panes"></a>
</p>

Conflicts open a three-way resolver: ours, result, theirs, with per-conflict buttons, accept-all and apply.

<p align="center">
  <a href="screenshot/gitgui-merge.png"><img src="screenshot/gitgui-merge.png" width="100%" alt="Three-way conflict resolver: ours, result, theirs"></a>
</p>

<p align="center">
  <a href="screenshot/gitgui-editor.png"><img src="screenshot/gitgui-editor.png" width="24%" alt="Built-in editor with syntax colors next to the sidebar"></a>
  <a href="screenshot/gitgui-branches.png"><img src="screenshot/gitgui-branches.png" width="24%" alt="Branch switcher"></a>
  <a href="screenshot/gitgui-menu.png"><img src="screenshot/gitgui-menu.png" width="24%" alt="Commit menu: cherry-pick, revert, reset, reword, squash, fixup, drop, move"></a>
  <a href="screenshot/gitgui-help.png"><img src="screenshot/gitgui-help.png" width="24%" alt="Keyboard reference"></a>
</p>
<p align="center">
  <sub>
    <a href="screenshot/gitgui-editor.png">editor</a> ·
    <a href="screenshot/gitgui-branches.png">branch switcher</a> ·
    <a href="screenshot/gitgui-menu.png">commit menu</a> ·
    <a href="screenshot/gitgui-help.png">shortcuts</a>
  </sub>
</p>

## Install options

Requires macOS or Linux and a kitty-graphics terminal (cmux, Ghostty, kitty, WezTerm). The one-liner at the top downloads a release binary into `~/.local/bin`, or builds from source with `cargo` when there is no binary for your platform.

```bash
GITGUI_VERSION=0.4.0 GITGUI_INSTALL_DIR=~/bin bash scripts/install.sh   # pin a version, other dir
cargo install --git https://github.com/antonellof/gitgui                 # from source (Rust 1.95+)
gitgui --probe                                                           # does this terminal support kitty graphics?
```

If `gitgui` is not found afterwards, add `~/.local/bin` to your `PATH`.

## Development

```
cargo test                              byte-exact tests for every protocol encoder and parser, git and UI harness tests
cargo clippy -- -D warnings
cargo run --release -- --headless-frame /tmp/frame.png --size 1600x1000 --scale 2 --open src/main.rs
                                        one PNG frame without a terminal, prints timings
GITGUI_HEADLESS_OPEN=merge cargo run --release -- --headless-frame /tmp/merge.png --repo scratch/conflict-demo
                                        same, with a dialog or the merge tool open (picker, help, menu, stash, reset, merge)
scripts/conflict-demo.sh                a throwaway repository with three conflicted files
scripts/graph-demo.sh                   a throwaway repository with branches, merges, tags, a remote, a stash and a conflict (the README screenshots)
bash scripts/smoke.sh                   headless smoke test
```

[docs/SPEC.md](docs/SPEC.md) is the source of truth for architecture and behavior, [docs/PROTOCOLS.md](docs/PROTOCOLS.md) for the exact escape sequences, [CLAUDE.md](CLAUDE.md) for pinned versions and API notes. Release binaries are built by `.github/workflows/release.yml` on a `v*` tag.

## License

MIT
