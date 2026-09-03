# gitgui plan and status

Living document. Updated at the end of every phase and whenever an issue is
found or closed. The milestones themselves are defined in SPEC.md section 8;
this file tracks where we are, what was decided, and what is open.

## Status

| Phase | State | Commit | Notes |
|---|---|---|---|
| 0 terminal plumbing | done | 22bef03 | probe, raw mode, kitty graphics shm + direct, restore on exit and panic |
| 1 rendering | done | 475c593 | rasterizer, framebuffer, egui demo, headless PNG |
| 2 input | done | eecddce | parser with byte tests, stdin thread, egui mapping, `--dump-input` |
| 3 read-only git | done | 9b7de3d | git2 0.21.0, snapshot + graph + diff, worker thread, real UI |
| 4 writes | next | | |
| 5 integration | | | |

## Measurements

Release build, raster stage only, `--headless-frame`:

| Frame | Time |
|---|---|
| 1600x1000 scale 1 | 2 ms |
| 1600x1000 scale 2 | 2.7 ms |
| 3344x1870 scale 2 (full cmux pane on the dev Mac) | 10 ms |

On screen in cmux (includes shm copy, encode, dirty check): about 7 ms per
frame at scale 2 in a half-width pane.

Git snapshot (status + 2000 commits + refs) on the worker thread:

| Repo | Time |
|---|---|
| gitgui (4 commits) | 6 ms |
| vllm (20k commits, capped at 2000) | 660 ms |

The UI never waits for it; the first frame shows "loading" until the
snapshot arrives. Filesystem polling every 2 s only re-snapshots when a
watched mtime changed.

## Decisions

- git2 0.21 returns `Result` from most string getters (`shorthand`,
  `summary`, `Signature::name`, `StatusEntry::path`); unwrap to empty strings.
- Rename detection: the status entry's `path()` is the old path, the delta's
  `new_file()` has the new one.
- The graph keeps the first parent in the commit's lane; merge curves are
  drawn on the row where lanes join, forks on the merge commit's row.
- Log virtualization uses `ScrollArea::show_rows`; 2000 rows cost nothing
  off screen.

- Shm probe rides in the capability batch as `a=q,t=s`; both an `OK` reply and
  the object being unlinked are required (PROTOCOLS 4.2).
- DECRQM for mode 1016 decides between pixel and cell mouse coordinates,
  instead of guessing from coordinate ranges.
- Ctrl acts as egui's `command` modifier, because terminals never deliver Cmd.
- Ctrl+C always quits. `q` quits only when no text field has keyboard focus.
- Copy goes out as OSC 52. There is no read path back from the clipboard;
  paste arrives through bracketed paste.
- Frame pacing: the loop sleeps on the input channel until the repaint delay
  egui asked for. Idle CPU is zero unless a widget requests animation.

## Open issues

- SSH manual check not done: Remote Login is off on the dev Mac. The direct
  transport was verified with `--no-shm` instead.
- `cmux send-key` does not deliver keys to the pane. `cmux send` types text
  through the terminal's key encoder (ESC arrives as `CSI 27 u`), so it can
  exercise the keyboard path but not the mouse path. Mouse checks use
  `scripts/click.swift`, which posts real CoreGraphics events.
- Terminal cursor shape cannot follow egui's cursor icon. Ignored.
- The demo app's spinner is off by default so an idle session uses no CPU.
  Turning it on repaints at 60 fps on purpose.

## Closed issues

- Phase 1: glyph rendering cost 17 ms of a 20 ms full-pane frame. Fixed with
  transparent texel skipping and nearest sampling for pixel-snapped glyphs.
- Phase 1: framebuffer `mark_sent` copied 12 MB per frame. Now swaps buffers.
