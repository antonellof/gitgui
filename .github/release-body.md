## Install

**Homebrew** (macOS and Linux):

```bash
brew tap antonellof/gitgui https://github.com/antonellof/gitgui
brew install antonellof/gitgui/gitgui
```

**Script** (puts the binary in `~/.local/bin`):

```bash
curl -fsSL https://raw.githubusercontent.com/antonellof/gitgui/main/scripts/install.sh | bash
```

**Manual**: take the tarball for your platform from the assets below, then

```bash
tar xzf gitgui-VERSION-macos-arm64.tar.gz
install -m 755 gitgui-VERSION-macos-arm64 ~/.local/bin/gitgui
```

## Update

```bash
brew update && brew upgrade gitgui                                                   # Homebrew
curl -fsSL https://raw.githubusercontent.com/antonellof/gitgui/main/scripts/install.sh | bash   # script
```

gitgui says so itself when a newer release is out: a chip in the footer, and a line on the terminal when you quit. `gitgui --check-update` asks right now, `--no-update-check` turns it off.

In-terminal rendering needs cmux, Ghostty, kitty or WezTerm; anywhere else the same binary opens a desktop window. `gitgui --probe` says which one you have.

---
