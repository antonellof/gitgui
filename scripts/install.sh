#!/usr/bin/env bash
# Install gitgui from GitHub releases, or build from source when needed.
set -euo pipefail

REPO="${GITGUI_REPO:-antonellof/gitgui}"
VERSION="${GITGUI_VERSION:-latest}"
INSTALL_DIR="${GITGUI_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="arm64" ;;
  *)
    echo "unsupported architecture: $arch" >&2
    exit 1
    ;;
esac
case "$os" in
  darwin) platform="macos" ;;
  linux) platform="linux" ;;
  *)
    echo "unsupported OS: $os" >&2
    exit 1
    ;;
esac

resolve_version() {
  if [ "$VERSION" != "latest" ]; then
    return
  fi
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    VERSION="$(gh release view --repo "$REPO" --json tagName -q '.tagName' 2>/dev/null | sed 's/^v//' || true)"
  fi
  if [ -z "${VERSION:-}" ]; then
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
      | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/p' \
      | head -1)" || VERSION=""
  fi
  if [ -z "${VERSION:-}" ]; then
    echo "no GitHub release found for ${REPO}; will build from source" >&2
    VERSION=""
  fi
}

install_from_release() {
  local asset="gitgui-${VERSION}-${platform}-${arch}"
  local tmpdir bin
  tmpdir="$(mktemp -d)"
  trap "rm -rf '${tmpdir}'" EXIT

  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    echo "downloading ${asset}.tar.gz from GitHub release v${VERSION} (via gh)"
    gh release download "v${VERSION}" --repo "$REPO" \
      --pattern "${asset}.tar.gz" --dir "$tmpdir" --clobber || return 1
    tar xzf "$tmpdir/${asset}.tar.gz" -C "$tmpdir"
  else
    local url="https://github.com/${REPO}/releases/download/v${VERSION}/${asset}.tar.gz"
    echo "downloading $url"
    curl -fsSL "$url" -o "$tmpdir/${asset}.tar.gz" || return 1
    tar xzf "$tmpdir/${asset}.tar.gz" -C "$tmpdir"
  fi

  bin="$tmpdir/${asset}"
  if [ ! -f "$bin" ]; then
    echo "release archive did not contain $asset" >&2
    return 1
  fi
  mkdir -p "$INSTALL_DIR"
  install -m 755 "$bin" "$INSTALL_DIR/gitgui"
  trap - EXIT
  rm -rf "$tmpdir"
}

install_from_source() {
  if ! command -v cargo >/dev/null; then
    echo "gitgui: no release binary available and cargo is not installed" >&2
    echo "install Rust from https://rustup.rs then re-run this script" >&2
    exit 1
  fi
  local tmpdir src
  tmpdir="$(mktemp -d)"
  src="$tmpdir/gitgui"
  trap "rm -rf '${tmpdir}'" EXIT
  echo "building gitgui from source (this takes a few minutes)"
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    if [ -n "${VERSION:-}" ]; then
      gh repo clone "$REPO" "$src" -- --depth 1 --branch "v${VERSION}"
    else
      gh repo clone "$REPO" "$src" -- --depth 1
    fi
  else
    if [ -n "${VERSION:-}" ]; then
      git clone --depth 1 --branch "v${VERSION}" "https://github.com/${REPO}.git" "$src"
    else
      git clone --depth 1 "https://github.com/${REPO}.git" "$src"
    fi
  fi
  mkdir -p "$INSTALL_DIR"
  cargo install --path "$src" --root "$INSTALL_DIR" --locked
  if [ -x "$INSTALL_DIR/bin/gitgui" ] && [ ! -e "$INSTALL_DIR/gitgui" ]; then
    mv "$INSTALL_DIR/bin/gitgui" "$INSTALL_DIR/gitgui"
    rmdir "$INSTALL_DIR/bin" 2>/dev/null || true
  fi
  trap - EXIT
  rm -rf "$tmpdir"
}

print_done() {
  echo ""
  echo "installed: $INSTALL_DIR/gitgui"
  if ! command -v gitgui >/dev/null 2>&1; then
    echo ""
    echo "add $INSTALL_DIR to your PATH, for example:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
  fi
  echo ""
  echo "run in a kitty-graphics terminal (cmux, Ghostty, kitty):"
  echo "  gitgui                  open repo in current directory"
  echo "  gitgui /path/to/repo    open a specific repo"
  echo "  gitgui --split right .  open in a new terminal split"
  echo ""
  echo "quit with q or Ctrl+C"
}

resolve_version

if [ -n "${VERSION:-}" ]; then
  if install_from_release; then
    print_done
    exit 0
  fi
  echo "release download failed; falling back to source build" >&2
fi

install_from_source
print_done
