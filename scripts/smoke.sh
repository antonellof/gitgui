#!/usr/bin/env bash
# End-to-end smoke test without a graphics terminal.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="${CARGO_TARGET_DIR:-target}/release/gitgui"
cargo build --release -q
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

git init "$TMP/repo" >/dev/null 2>&1
git -C "$TMP/repo" config user.email "test@example.com"
git -C "$TMP/repo" config user.name "Test"
echo hello >"$TMP/repo/readme.txt"
git -C "$TMP/repo" add readme.txt
git -C "$TMP/repo" commit -m "initial" >/dev/null

echo "== probe =="
"$BIN" --probe >/dev/null || true

echo "== headless frame =="
"$BIN" --headless-frame "$TMP/frame.png" --repo "$TMP/repo" --size 800x600
test -s "$TMP/frame.png"

echo "== agent ls =="
"$BIN" ls

echo "smoke ok"
