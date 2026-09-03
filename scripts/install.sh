#!/usr/bin/env bash
# Install gitgui from GitHub releases.
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
  darwin) asset="gitgui-${VERSION}-macos-${arch}" ;;
  linux) asset="gitgui-${VERSION}-linux-${arch}" ;;
  *)
    echo "unsupported OS: $os" >&2
    exit 1
    ;;
esac

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/${REPO}/releases/latest/download/${asset}.tar.gz"
else
  url="https://github.com/${REPO}/releases/download/v${VERSION}/${asset}.tar.gz"
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
echo "downloading $url"
curl -fsSL "$url" -o "$tmpdir/archive.tar.gz"
tar xzf "$tmpdir/archive.tar.gz" -C "$tmpdir"
bin="$(find "$tmpdir" -maxdepth 1 -type f -name 'gitgui-*' | head -1)"
test -n "$bin"
mkdir -p "$INSTALL_DIR"
install -m 755 "$bin" "$INSTALL_DIR/gitgui"
echo "installed $INSTALL_DIR/gitgui"
if ! command -v gitgui >/dev/null 2>&1; then
  echo "add $INSTALL_DIR to your PATH"
fi
