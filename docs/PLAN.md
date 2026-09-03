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
| 4 writes | done | b4de578 | stage/unstage files and hunks, commit, amend, checkout, branches, stash, discard, fetch/pull/push via git CLI |
| 5 integration | done | | split.rs, agent.rs, skill/SKILL.md, install script, release workflow |

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

- Unstaging a hunk reverses the HEAD-to-index patch text (swap hunk ranges
  and +/- prefixes, keep the file headers) and applies it to the index.
- Network ops run on the git worker thread through the git CLI with
  `GIT_TERMINAL_PROMPT=0`; output streams into a log panel. A slow push blocks
  other git commands until it finishes, the UI stays responsive.
- `commit.gpgsign=true` routes commits through `git commit` so signing works.
- Manual testing on this machine: never inject mouse events while the user
  works; `cmux send` targets a specific surface and is safe, screenshots are
  only meaningful when the gitgui workspace is in front.

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

## User requests (2026-09-03)

- UI must be usable at the terminal's own text size: font size now derives
  from the cell height (`cell_h / ppp * 0.76`), override with `--font-size`.
  Loading the terminal's font family itself (Ghostty `font-family`, resolved
  through CoreText / fontconfig) is a follow-up; egui's bundled fonts are
  used until then.
- A status bar at the bottom listing the available commands: present, and
  visible now that the scale bug is fixed.
- cmux panes resize and move between displays: SIGWINCH re-reads the grid
  from the ioctl, re-queries the cell size in-band (`CSI 16 t`), and when the
  cell height changes the scale and font size follow. Window resizes while a
  frame is in flight are coalesced by the identical-frame skip.

## Next steps (resume here)

1. Fonts: load the terminal's font family (Ghostty `font-family`) via a
   system font lookup; currently egui's bundled fonts at the terminal's size.
2. Manual verification still owed on a real screen: commit via the Commit
   button, Stage hunk button, branch context menus, fetch/pull/push log,
   short-pane layout (detail pane about 225 pt tall). Harness tests cover
   the commit button; layout math is unit-tested in `changes.rs`.
3. First tagged release (`v0.1.0`) to populate GitHub release binaries for
   the install script. README demo recording still open.
4. Commit Phase 5 integration (split, agent, skill, install script, release
   workflow) plus the layout fix when ready.

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

- Detail column overlapped the commit box in short panes (~225 pt): nested
  `Panel::bottom` did not clip the lists above it. Fixed with explicit
  height allocation (`allocate_ui_with_layout`) and shrinking commit rows.

- A gitgui whose pane was closed kept running at 100% CPU: the stdin thread
  treated EOF like a timeout and spun, and the main loop could sleep up to an
  hour before looking at the SIGHUP flag. EOF now ends the thread (the loop
  exits on the closed channel) and the idle wait is capped at 500 ms.

- Phase 3/4: on screen the UI was drawn at 4x (2x too big), clicks landed on
  the wrong widgets and the status bar was off screen. Cause: egui's
  `pixels_per_point = zoom_factor * native_pixels_per_point`; the runtime set
  the zoom via `set_pixels_per_point` AND passed the native scale. Only the
  native scale is passed now, with a regression test.

- Phase 1: glyph rendering cost 17 ms of a 20 ms full-pane frame. Fixed with
  transparent texel skipping and nearest sampling for pixel-snapped glyphs.
- Phase 1: framebuffer `mark_sent` copied 12 MB per frame. Now swaps buffers.
