# scripts

Developer helpers for manual checks. Not part of the binary.

- `click.swift`: posts real mouse events with CoreGraphics so a running gitgui
  can be clicked, dragged and scrolled from a script. Build with
  `swiftc -O -o /tmp/click scripts/click.swift`, then
  `/tmp/click <x> <y> [move|click|down|up|drag|rclick|scroll <lines>]` in
  logical screen coordinates. Needs Accessibility permission for the terminal.
  Terminals encode typed text, so raw mouse escape sequences cannot be injected
  with `cmux send`; this tool is the way to exercise the mouse path.
