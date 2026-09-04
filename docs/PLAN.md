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
| 5 integration | done | b394bdc | split.rs, agent.rs, skill/SKILL.md, install script, release workflow |
| post-v0.1 polish | done | ccdf818 | v0.1.1 to v0.1.4: keyboard shortcuts, agent network ops, auto refresh, footer toolbar, branch picker, GitHub publish |
| review pass | done | (this commit) | 2026-09-04 audit: layout fixes for short and narrow panes, socket permissions, bounds checks; see "Review 2026-09-04" |

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

## Review 2026-09-04

Full pass over the code after v0.1.4 (three review agents plus headless
frames at 800x500, 550x350 and 1672x935 pt). Clippy and 108 tests green
before and after.

### Fixed in this pass

Layout (all visible in `--headless-frame` at 1600x1000 scale 2 before the fix):

- Footer text (path, branch, counts) overlapped the Quit and Refresh buttons
  in panes under about 900 pt. Cause: the trailing `right_to_left` layout
  was allocated after the leading labels and simply overflowed to the left.
  New `ui/row.rs` helper lays out the trailing widgets first and clips the
  leading side to what is left; used by the footer, the diff header and the
  commit button row. The footer drops the path first, then truncates the
  counts, so the branch switcher always stays visible.
- Commit box vanished below the footer in short detail panes. Cause: nested
  `allocate_ui_with_layout` grows past the requested height when the lists
  overflow, and `ScrollArea` has a 64 pt minimum. The lists and the commit
  box now get hard rects (`scope_builder` + `max_rect` + clip) and the
  scroll areas use `min_scrolled_height(0)`. During egui's panel sizing
  pass the detail pane reports a fixed 260 pt so the bottom panel does not
  balloon to the whole window.
- Commit summaries wrapped to two lines and clipped inside a single row.
  Now `layout_no_wrap` with the existing clip rect.
- Hunk header text ran underneath the `Stage hunk` button. Text is clipped
  to the space left of the button; in wrap mode the wrap width reserves the
  button too.
- Empty "nothing staged" list took half of the list column while the
  unstaged list showed one row. `list_scroll_heights` now takes row counts;
  a list only gets what its rows need, the rest goes to the other list.
- Toolbar wider than the footer in narrow panes (Quit wrapped off screen).
  Below 560 pt the fetch/pull/push/refresh buttons show icons only.
- Sidebar defaulted to 220 pt even in a 550 pt pane. Default is now 25 % of
  the width, clamped to 140..220.
- Amend checkbox label truncated to a stray dot in narrow columns; the label
  is dropped below 72 pt (hover text still explains it).

Robustness:

- Agent socket directory and socket file are now chmod 0700 / 0600. Before,
  another local user could drive the repository through `$TMPDIR/gitgui`.
- `--size WxH` rejects 0 and anything above 16384 instead of allocating a
  zero or multi-gigabyte framebuffer.
- Texture atlas partial update with an x offset past the atlas width no
  longer indexes out of bounds.
- Commit list row drawing uses `get` instead of indexing the commits vec, so
  a stale selection during a snapshot swap cannot panic.
- Filter changes kept `Selection::Commit(i)` on a commit the filter hid (or
  on the working tree row, which a filter hides); the next `j` / `k` jumped
  to the top. `rebuild_filter` now moves the selection to the first visible
  row when the current one disappears.

### Reviewed and left as is

- Bracketed paste ends at the literal `ESC [ 201 ~`; pasted text containing
  that sequence is cut short. Protocol limitation, no escaping exists.
- Unstaging in an empty repository ignores `remove_path` / `remove_dir`
  errors: exactly one of the two applies per path, the other always fails.
- Hunk staging skips binary deltas; the UI never offers hunk buttons for a
  binary diff, so nothing is silently lost.
- Untracked files above 2 MB are read once to decide `too_large`. Cheap
  enough, only happens when the file is selected.
- Screenshot path from the agent socket is not sandboxed. The socket is
  owner-only now, and the agent runs as the same user anyway.

### Open findings

- Detail pane under about 200 pt: the staged header gets clipped by the
  commit box. Acceptable, but hiding the staged section when it does not fit
  would look cleaner.
- Ghostty `font-family` is still not loaded; egui bundled fonts.
- `probe.rs` parses the reply buffer twice (once to check DA arrival, once
  for real). Harmless.
- Test gaps: paste containing ESC, kitty super modifier, malformed SGR
  mouse, escape split at odd byte boundaries, atlas update past bounds.

## Next steps (resume here)

### Post v0.1 feature backlog

Priority order for the next releases. Sizes: small = an evening, medium = a
few sessions, large = a milestone.

1. **Diff search** (medium): `/` or `Ctrl+F` in the diff pane, `n` / `N`
   for next match, match highlight. High value when reviewing large patches.
2. **Conflict UI** (large): show the merge / rebase state in the footer,
   list conflicted files, pick ours / theirs per hunk, mark resolved,
   `git merge --abort` / `rebase --abort` buttons. Needed for merge-heavy
   workflows and the most requested gap from the review.
3. **Commit context menu** (medium): cherry-pick, revert, create tag, create
   branch here, copy hash, reset soft / mixed / hard with confirmation. All
   plain `git2` calls, the UI is the work.
4. **Terminal fonts** (medium): load Ghostty `font-family` via CoreText /
   fontconfig so UI text matches the terminal face at the computed size.
5. **File history and blame** (medium): `h` on a file opens its log,
   `b` opens blame in the diff pane. Read-only, `git2` has both.
6. **tmux / Zellij passthrough** (medium): detect and enable kitty graphics
   passthrough instead of hard exit.
7. **Remote management** (small): add / remove / rename remotes from the
   sidebar context menu; `Ctrl+U` to set upstream on push.
8. **Repo switcher** (small): recent repos list, `--repo` history file
   under XDG config.
9. **README demo** (small): screen recording for the install section.

Not planned for v0.x: interactive rebase, bisect, submodules, worktrees,
sparse checkout, patch export. Use the git CLI in the neighbouring pane.

### Recently shipped (v0.1.1 to v0.1.4)

- v0.1.4: footer toolbar with fetch / pull / push / refresh / quit, branch
  picker with dirty-tree confirm, publish to GitHub via `gh`, init screen
  for non-git folders, compact commit box, wrap-aware diff row heights
- v0.1.3 and earlier:

- Install script fix (binary vs tar.gz)
- Commit and push button and worker command
- Status bar key letter highlighting
- Keyboard: `Shift+S` stash, `Ctrl+Shift+Enter` commit and push, `r` refresh (manual)
- Agent API: `fetch`, `pull`, `push`, `commit_and_push`
- **Auto refresh**: git worker polls every 2 s; refreshes when worktree fingerprint or `.git/` mtimes change, even when the gitgui pane is unfocused (edits from pi in a neighboring cmux pane show up without pressing `r`)

### Manual verification still owed

Commit via button, Stage hunk, branch context menus, fetch/pull/push log,
branch picker, publish to GitHub. The 2026-09-04 layout fixes were verified
with headless frames at three sizes; a real cmux pane check of the narrow
(icon-only) footer and the short detail pane is still owed. Harness tests
cover the commit button; layout math is unit-tested in `changes.rs`.

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
