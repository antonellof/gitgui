# scripts

Developer helpers for manual checks. Not part of the binary.

- `install.sh`: downloads a release binary from GitHub into `~/.local/bin`, or builds from source when no release exists (private repos, first install). Use `gh repo clone` + `bash scripts/install.sh` for private repos; the raw `curl | bash` one-liner only works on public repos.
- `smoke.sh`: runs `--probe`, `--headless-frame`, and `gitgui ls` against a fixture repo.
- `click.swift`: posts real mouse events with CoreGraphics so a running gitgui
  can be clicked, dragged and scrolled from a script. Build with
  `swiftc -O -o /tmp/click scripts/click.swift`, then
  `/tmp/click <x> <y> [move|click|down|up|drag|rclick|scroll <lines>]` in
  logical screen coordinates. Needs Accessibility permission for the terminal.
  Terminals encode typed text, so raw mouse escape sequences cannot be injected
  with `cmux send`; this tool is the way to exercise the mouse path.

