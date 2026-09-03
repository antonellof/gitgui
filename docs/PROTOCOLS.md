# Terminal protocols used by gitgui

Exact sequences. `ESC` is 0x1B, `CSI` is `ESC [`, `APC` is `ESC _`, `ST` is `ESC \`. All numbers are decimal ASCII.

## 1. Session setup and teardown

Setup, in this order:

```
CSI ? 1049 h        alternate screen
CSI ? 25 l          hide cursor
CSI ? 1004 h        focus events
CSI ? 1003 h        report all mouse motion
CSI ? 1006 h        SGR mouse encoding
CSI ? 1016 h        SGR mouse in pixel coordinates (overrides 1006 cell coords when supported)
CSI ? 2004 h        bracketed paste
CSI > 15 u          push kitty keyboard flags: 1 disambiguate, 2 event types, 4 alternate keys, 8 all keys as escapes
```

Teardown, reverse order:

```
CSI < u             pop kitty keyboard flags
CSI ? 2004 l
CSI ? 1016 l
CSI ? 1006 l
CSI ? 1003 l
CSI ? 1004 l
CSI ? 25 h
CSI ? 1049 l
```

Before teardown, delete all image placements (section 4.5) so nothing leaks into the main screen scrollback.

Raw mode via termios: clear `ICANON | ECHO | ISIG | IEXTEN` in `c_lflag`, clear `IXON | ICRNL | BRKINT | INPCK | ISTRIP` in `c_iflag`, clear `OPOST` in `c_oflag`, set `VMIN=0 VTIME=0` and poll stdin with `poll(2)`. Keep `ISIG` off and handle Ctrl+C ourselves as a quit key.

Synchronized output around each frame so the terminal composes text and image atomically:

```
CSI ? 2026 h   ... frame sequences ...   CSI ? 2026 l
```

## 2. Capability probes

Send all probes at once, then read responses until the `CSI c` (primary DA) reply arrives; DA is always answered, so it terminates the probe.

```
APC G i=31,s=1,v=1,a=q,t=d,f=24 ; AAAA ST     kitty graphics probe (1x1 RGB pixel)
APC G i=32,s=1,v=1,a=q,t=s,f=32 ; <b64 name> ST shared memory probe (section 4.2)
CSI ? u                                       kitty keyboard probe
CSI ? 1016 $ p                                DECRQM: does the terminal know pixel mouse mode?
CSI 16 t                                      cell size in pixels
CSI 14 t                                      text area size in pixels
CSI 18 t                                      text area size in cells
CSI c                                         primary device attributes (terminator)
```

Expected replies:

```
APC G i=31 ; OK ST                 graphics supported (any other message or no reply: unsupported)
APC G i=32 ; OK ST                 shm transport supported (see 4.2, sent only when not over SSH)
CSI ? <flags> u                    kitty keyboard supported, current flags
CSI ? 1016 ; <Ps> $ y              DECRPM: Ps 0 = mode unknown (mouse reports cells), 1..4 = known (pixels once enabled)
CSI 6 ; <height> ; <width> t       cell size
CSI 4 ; <height> ; <width> t       pixel size
CSI 8 ; <rows> ; <cols> t          cell grid
CSI ? 6 ... c                      DA reply, discard
```

Also read `TIOCGWINSZ` (`ws_row, ws_col, ws_xpixel, ws_ypixel`). Ghostty and kitty fill the pixel fields. Prefer the ioctl for resize handling (SIGWINCH) and fall back to `CSI 14 t` when the pixel fields are zero.

HiDPI: pixel sizes reported are device pixels. Derive `pixels_per_point` as `cell_height_px / 16.0` clamped to `{1.0, 1.5, 2.0}` and expose `--scale` to override. Font atlas quality depends on this value, so recompute when the cell size changes.

Framebuffer size in pixels is `cols * cell_w` by `rows * cell_h`. Never use the raw window pixel size; the terminal only lets an image occupy the cell grid.

## 3. Input decoding

Read bytes from stdin into a ring buffer. Parse greedily; if a sequence is incomplete, wait for more bytes up to 50 ms, then treat a lone `ESC` as the Escape key.

### 3.1 Kitty keyboard protocol

```
CSI <unicode-key> [ : <shifted-key> [ : <base-layout-key> ] ] ; <modifiers> [ : <event-type> ] [ ; <text-codepoints> ] u
```

- `modifiers` is `1 + bits`: shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32, caps_lock 64, num_lock 128. Absent means 1.
- `event-type`: 1 press (default), 2 repeat, 3 release. Map press and repeat to `KeyDown`, release to `KeyUp`.
- Functional keys arrive as private use codepoints: Escape 27, Enter 13, Tab 9, Backspace 127, Insert 57348, Delete 57349, Left 57350, Right 57351, Up 57352, Down 57353, PageUp 57354, PageDown 57355, Home 57356, End 57357, F1..F12 57364..57375. Also accept the legacy forms below because some keys still use them even with flags pushed.
- Legacy forms to accept: `CSI A/B/C/D` arrows, `CSI 1 ; <mod> A` etc., `CSI H`/`CSI F` home/end, `CSI 2~ 3~ 5~ 6~` insert/delete/pgup/pgdn, `CSI 3 ; <mod> ~`.
- Text: when the `text-codepoints` field is present, use it as the typed text. Otherwise, for a press of a printable `unicode-key` without ctrl/alt/super, the text is that codepoint; with shift held use the `shifted-key` field when present (flag 4 makes the terminal send it, so `shift+1` yields `!`), else uppercase the key if it is ASCII.
- Modifier keys themselves (left shift 57441 and friends) arrive as key events because of flag 8. They map to no key and no text.
- Lock modifiers (caps lock 64, num lock 128) are masked out of the modifier bits.

Fallback when kitty keyboard is unsupported: plain UTF-8 bytes are text, `0x01..0x1A` are `Ctrl+<letter>`, `ESC <byte>` is `Alt+<byte>`.

### 3.2 SGR mouse

```
CSI < <Cb> ; <Px> ; <Py> M     press or motion
CSI < <Cb> ; <Px> ; <Py> m     release
```

- With `?1016h` active, `Px, Py` are 1-based pixels. Convert to 0-based by subtracting 1. If the DECRQM probe reported mode 1016 as unknown (Ghostty accepted it in 2025, older builds did not), coordinates are cells: convert with `(Px - 1) * cell_w + cell_w / 2`.
- `Cb` low two bits: 0 left, 1 middle, 2 right, 3 none (motion with no button when combined with 32).
- `Cb` bit 4 shift, bit 8 alt, bit 16 ctrl, bit 32 motion, bit 64 wheel: 64 wheel up, 65 wheel down, 66 wheel left, 67 wheel right (high-resolution terminals send these repeatedly; each event is one notch, translate to 40 px of scroll per notch in egui points, adjusted by scale).
- Ghostty trackpad scrolling arrives as many wheel events per second. Coalesce wheel events received within one poll cycle into a single egui scroll delta.

### 3.3 Other

```
CSI I   focus gained
CSI O   focus lost
CSI 200 ~  ...  CSI 201 ~    bracketed paste, deliver as one Text event
```

SIGWINCH: set an atomic flag from the handler, re-run the size probes on the next loop iteration, reallocate the framebuffer, delete old placements, request a full repaint.

## 4. Kitty graphics protocol

General form:

```
APC G <key>=<value>,<key>=<value>,... ; <base64 payload> ST
```

Always include `q=2` to suppress all responses so we do not have to drain acknowledgements from stdin.

### 4.1 Image data format

`f=32` raw RGBA, 8 bits per channel, row-major, no padding, straight alpha. Since we paint an opaque background every frame, alpha is always 255 and premultiplication is irrelevant.

### 4.2 Shared memory transport (local)

1. `shm_open("/tg-<pid>-<seq>", O_CREAT | O_RDWR | O_EXCL, 0600)`, `ftruncate` to `w * h * 4`, `mmap`, copy the framebuffer, `munmap`, close the fd. Do NOT `shm_unlink`; the terminal unlinks it after reading.
2. Send:

```
APC G a=T,t=s,f=32,s=<w>,v=<h>,i=<id>,p=<pid>,C=1,q=2 ; <base64("/tg-<pid>-<seq>")> ST
```

`a=T` transmits and displays in one command. `C=1` keeps the cursor where it is. Move the cursor to row 1 column 1 (`CSI 1 ; 1 H`) before the command so the image origin is the top-left cell.

On macOS the shm name must be at most 31 characters including the leading slash. Keep the name short.

Probe the transport once at startup, as part of the capability batch in section 2: create a 1x1 RGBA object named `/tg-<pid>-p` and send

```
APC G i=32,s=1,v=1,a=q,t=s,f=32 ; <base64("/tg-<pid>-p")> ST
```

`a=q` makes the terminal load the data without storing or displaying it. The transport works when both hold: the reply is `APC G i=32 ; OK ST`, and the object is gone afterwards (`shm_open` without `O_CREAT` fails), because a terminal that read the object unlinks it. If the object still exists after the DA reply arrived, unlink it ourselves and use the direct transport.

### 4.3 Direct transport (SSH or fallback)

Compress the RGBA buffer with zlib (`flate2`, level 1 is enough), base64-encode, and split into chunks of at most 4096 base64 characters:

```
APC G a=T,t=d,o=z,f=32,s=<w>,v=<h>,i=<id>,p=<pid>,C=1,q=2,m=1 ; <chunk 1> ST
APC G m=1 ; <chunk 2> ST
...
APC G m=0 ; <last chunk> ST
```

Only the first chunk carries the control keys. Throttle to 20 fps on this path and rely on identical-frame skipping.

### 4.4 Double buffering without flicker

Alternate between two image ids, 1 and 2. Frame N goes to id `1 + (N % 2)`, placement id equal to the image id. Sequence per frame:

```
CSI ? 2026 h
CSI 1 ; 1 H
APC G a=T,i=<new>,p=<new>,... ST          new frame appears on top
APC G a=d,d=i,i=<old>,p=<old>,q=2 ST      old placement removed
CSI ? 2026 l
```

Because both placements share the same origin and size, the swap is invisible.

### 4.5 Cleanup

Before leaving the alternate screen:

```
APC G a=d,d=A,q=2 ST      delete all placements and free all image data
```

### 4.6 Layering with text

We draw nothing as text; the whole cell grid is covered by the image. Default `z=0` is fine. If a future feature wants terminal text on top of the image, place the image with `z=-1` (below text) and leave the covered cells blank.

## 5. Terminal color query (optional, phase 3)

```
OSC 10 ; ? ST      foreground   -> OSC 10 ; rgb:rrrr/gggg/bbbb ST
OSC 11 ; ? ST      background   -> OSC 11 ; rgb:rrrr/gggg/bbbb ST
```

Use the background to pick a light or dark theme and to blend the UI with the surrounding panes. Time out after 100 ms and fall back to dark.

## 6. Multiplexer notes

- tmux: kitty graphics work only with `allow-passthrough on` and every APC wrapped in a DCS passthrough (`ESC P tmux ; ESC <sequence with ESC doubled> ESC \`). Out of scope for MVP; detect `TMUX` and print a clear message.
- cmux and Ghostty: native support, no wrapping.
- Zellij: unsupported at the time of writing, print a clear message.
