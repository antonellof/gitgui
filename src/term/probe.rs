//! Terminal capability probing (docs/PROTOCOLS.md section 2).
//!
//! All probes are written at once; replies are read until the primary DA
//! answer arrives, which every terminal sends. Reply parsing is pure and unit
//! tested with literal bytes.

use std::fmt;
use std::time::{Duration, Instant};

use super::kitty;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Terminal answered the kitty graphics query with OK.
    pub kitty_graphics: bool,
    /// Terminal answered the shm probe with OK (shared memory transport works).
    pub shm: bool,
    /// Kitty keyboard protocol flags currently active, if supported.
    pub kitty_keyboard: Option<u32>,
    /// Terminal recognises mode 1016 (SGR mouse in pixels), per DECRQM.
    pub pixel_mouse: bool,
    /// Cell size in device pixels.
    pub cell_w: u32,
    pub cell_h: u32,
    /// Text area size in device pixels as reported by the terminal.
    pub px_w: u32,
    pub px_h: u32,
    /// Cell grid.
    pub rows: u32,
    pub cols: u32,
    /// Terminal background color from OSC 11, if answered.
    pub background: Option<[u8; 3]>,
    /// Raw primary DA reply parameters, for diagnostics.
    pub da: String,
    /// Environment hints.
    pub ssh: bool,
    pub term_program: String,
    pub multiplexer: Option<String>,
}

impl Capabilities {
    /// Framebuffer size: the cell grid in pixels, never the raw window size.
    pub fn frame_size(&self) -> (u32, u32) {
        (self.cols * self.cell_w, self.rows * self.cell_h)
    }

    /// `cell_h / 16` clamped to {1, 1.5, 2}.
    pub fn pixels_per_point(&self) -> f32 {
        let raw = self.cell_h as f32 / 16.0;
        if raw < 1.25 {
            1.0
        } else if raw < 1.75 {
            1.5
        } else {
            2.0
        }
    }
}

impl fmt::Display for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "kitty graphics : {}", self.kitty_graphics)?;
        writeln!(f, "shm transport  : {}", self.shm)?;
        match self.kitty_keyboard {
            Some(flags) => writeln!(f, "kitty keyboard : yes (flags {flags})")?,
            None => writeln!(f, "kitty keyboard : no")?,
        }
        writeln!(f, "pixel mouse    : {}", self.pixel_mouse)?;
        writeln!(f, "cell size      : {}x{} px", self.cell_w, self.cell_h)?;
        writeln!(f, "text area      : {}x{} px", self.px_w, self.px_h)?;
        writeln!(
            f,
            "grid           : {} cols x {} rows",
            self.cols, self.rows
        )?;
        let (fw, fh) = self.frame_size();
        writeln!(
            f,
            "frame          : {fw}x{fh} px, scale {}",
            self.pixels_per_point()
        )?;
        match self.background {
            Some([r, g, b]) => writeln!(f, "background     : #{r:02x}{g:02x}{b:02x}")?,
            None => writeln!(f, "background     : unknown")?,
        }
        writeln!(f, "primary DA     : {}", self.da)?;
        writeln!(f, "ssh            : {}", self.ssh)?;
        writeln!(f, "TERM_PROGRAM   : {}", self.term_program)?;
        match &self.multiplexer {
            Some(m) => writeln!(f, "multiplexer    : {m}"),
            None => writeln!(f, "multiplexer    : none"),
        }
    }
}

/// Parse every reply present in `buf`. Returns true when the DA reply was
/// seen, meaning no more replies are expected.
pub fn parse_replies(buf: &[u8], caps: &mut Capabilities) -> bool {
    let mut saw_da = false;
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != 0x1b {
            i += 1;
            continue;
        }
        let rest = &buf[i + 1..];
        if rest.starts_with(b"]") {
            // OSC ... (ST | BEL)
            let body = &rest[1..];
            let st = body.windows(2).position(|w| w == b"\x1b\\");
            let bel = body.iter().position(|&c| c == 0x07);
            let (end, skip) = match (st, bel) {
                (Some(s), Some(l)) if l < s => (l, 1),
                (Some(s), _) => (s, 2),
                (None, Some(l)) => (l, 1),
                (None, None) => break,
            };
            parse_osc_reply(&body[..end], caps);
            i += 1 + 1 + end + skip;
        } else if rest.starts_with(b"_G") {
            // APC G ... ST
            let Some(end) = find(rest, b"\x1b\\") else {
                break;
            };
            parse_graphics_reply(&rest[2..end], caps);
            i += 1 + end + 2;
        } else if rest.first() == Some(&b'[') {
            // CSI params final
            let body = &rest[1..];
            let Some(fin) = body.iter().position(|b| (0x40..=0x7e).contains(b)) else {
                break;
            };
            let params = &body[..fin];
            match body[fin] {
                b'u' => {
                    if let Some(p) = params.strip_prefix(b"?") {
                        caps.kitty_keyboard =
                            std::str::from_utf8(p).ok().and_then(|s| s.parse().ok());
                    }
                }
                b't' => {
                    let nums: Vec<u32> = std::str::from_utf8(params)
                        .unwrap_or("")
                        .split(';')
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if nums.len() == 3 {
                        match nums[0] {
                            6 => {
                                caps.cell_h = nums[1];
                                caps.cell_w = nums[2];
                            }
                            4 => {
                                caps.px_h = nums[1];
                                caps.px_w = nums[2];
                            }
                            8 => {
                                caps.rows = nums[1];
                                caps.cols = nums[2];
                            }
                            _ => {}
                        }
                    }
                }
                b'y' if params.starts_with(b"?") && params.ends_with(b"$") => {
                    // DECRPM: CSI ? Pd ; Ps $ y   (Ps 0 = not recognised)
                    let inner = &params[1..params.len() - 1];
                    let mut it = inner.split(|&c| c == b';');
                    let mode = it
                        .next()
                        .and_then(|s| std::str::from_utf8(s).ok())
                        .and_then(|s| s.parse::<u32>().ok());
                    let state = it
                        .next()
                        .and_then(|s| std::str::from_utf8(s).ok())
                        .and_then(|s| s.parse::<u32>().ok());
                    if mode == Some(1016) {
                        caps.pixel_mouse = matches!(state, Some(1..=4));
                    }
                }
                b'c' if params.starts_with(b"?") => {
                    caps.da = String::from_utf8_lossy(params).into_owned();
                    saw_da = true;
                }
                _ => {}
            }
            i += 1 + 1 + fin + 1;
        } else {
            i += 1;
        }
    }
    saw_da
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `11;rgb:rrrr/gggg/bbbb` (16 bit per channel) or `11;rgb:rr/gg/bb`.
fn parse_osc_reply(body: &[u8], caps: &mut Capabilities) {
    let text = String::from_utf8_lossy(body);
    let Some(rest) = text.strip_prefix("11;") else {
        return;
    };
    let Some(rgb) = rest.strip_prefix("rgb:") else {
        return;
    };
    let parts: Vec<&str> = rgb.split('/').collect();
    if parts.len() != 3 {
        return;
    }
    let mut out = [0u8; 3];
    for (i, p) in parts.iter().enumerate() {
        let Ok(v) = u32::from_str_radix(p, 16) else {
            return;
        };
        out[i] = match p.len() {
            1 => (v * 17) as u8,
            2 => v as u8,
            3 => (v >> 4) as u8,
            _ => (v >> 8) as u8,
        };
    }
    caps.background = Some(out);
}

fn parse_graphics_reply(body: &[u8], caps: &mut Capabilities) {
    let text = String::from_utf8_lossy(body);
    let Some((ctl, msg)) = text.split_once(';') else {
        return;
    };
    let id: Option<u32> = ctl
        .split(',')
        .find_map(|kv| kv.strip_prefix("i="))
        .and_then(|v| v.parse().ok());
    let ok = msg.trim() == "OK";
    match id {
        Some(kitty::PROBE_IMAGE_ID) => caps.kitty_graphics = ok,
        Some(kitty::SHM_PROBE_IMAGE_ID) => caps.shm = ok,
        _ => {}
    }
}

/// Fill the environment-derived fields.
pub fn env_hints(caps: &mut Capabilities) {
    caps.ssh =
        std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    caps.term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    caps.multiplexer = if std::env::var_os("TMUX").is_some() {
        Some("tmux".into())
    } else if std::env::var_os("ZELLIJ").is_some() {
        Some("zellij".into())
    } else {
        None
    };
}

/// Window size from `TIOCGWINSZ`: (rows, cols, xpixel, ypixel).
pub fn winsize() -> Option<(u32, u32, u32, u32)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: ws is a valid out pointer for TIOCGWINSZ.
    let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if rc != 0 {
        return None;
    }
    Some((
        ws.ws_row as u32,
        ws.ws_col as u32,
        ws.ws_xpixel as u32,
        ws.ws_ypixel as u32,
    ))
}

/// Refresh grid and cell size after SIGWINCH using the ioctl. Keeps the
/// probed cell size when the pixel fields are zero.
pub fn apply_winsize(caps: &mut Capabilities) {
    if let Some((rows, cols, xp, yp)) = winsize() {
        if rows > 0 && cols > 0 {
            caps.rows = rows;
            caps.cols = cols;
            if xp > 0 && yp > 0 {
                caps.px_w = xp;
                caps.px_h = yp;
                caps.cell_w = xp / cols;
                caps.cell_h = yp / rows;
            }
        }
    }
}

/// Run the probe sequence. The terminal must already be in raw mode with echo
/// off, otherwise the replies are echoed to the screen. `shm_probe` sends the
/// shared memory probe when true.
pub fn probe(shm_probe: bool, timeout: Duration) -> std::io::Result<Capabilities> {
    let mut caps = Capabilities::default();
    env_hints(&mut caps);

    let mut out = Vec::new();
    kitty::encode_query_probe(&mut out);
    let shm = if shm_probe && !caps.ssh {
        let name = format!("/tg-{}-p", std::process::id());
        match kitty::Shm::create_and_fill(&name, &[0, 0, 0, 255]) {
            Ok(shm) => {
                kitty::encode_shm_probe(&mut out, &name);
                Some(shm)
            }
            Err(_) => None,
        }
    } else {
        None
    };
    out.extend_from_slice(b"\x1b[?u\x1b[?1016$p\x1b]11;?\x1b\\\x1b[16t\x1b[14t\x1b[18t\x1b[c");
    super::write_all(&out)?;

    let deadline = Instant::now() + timeout;
    let mut buf = Vec::with_capacity(256);
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let mut chunk = [0u8; 256];
        match super::read_timeout(&mut chunk, deadline - now) {
            Ok(0) => continue,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(e),
        }
        if parse_replies(&buf, &mut Capabilities::default()) {
            break;
        }
    }
    parse_replies(&buf, &mut caps);
    if let Some(shm) = shm {
        // The terminal unlinks the object when it consumed it. If it is still
        // there the transport is not usable regardless of the reply.
        if shm.exists() {
            caps.shm = false;
            shm.unlink();
        }
    }
    apply_winsize(&mut caps);
    if caps.cell_w == 0 || caps.cell_h == 0 {
        if let Some((rows, cols, _, _)) = winsize() {
            caps.rows = rows;
            caps.cols = cols;
        }
    }
    Ok(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_replies_ghostty_style() {
        let bytes = b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\\x1b[?1u\x1b[?1016;2$y\x1b[6;32;14t\x1b[4;1000;1600t\x1b[8;31;114t\x1b[?62;22c";
        let mut caps = Capabilities::default();
        assert!(parse_replies(bytes, &mut caps));
        assert!(caps.kitty_graphics);
        assert!(caps.shm);
        assert!(caps.pixel_mouse);
        assert_eq!(caps.kitty_keyboard, Some(1));
        assert_eq!((caps.cell_w, caps.cell_h), (14, 32));
        assert_eq!((caps.px_w, caps.px_h), (1600, 1000));
        assert_eq!((caps.cols, caps.rows), (114, 31));
        assert_eq!(caps.da, "?62;22");
        assert_eq!(caps.frame_size(), (114 * 14, 31 * 32));
        assert_eq!(caps.pixels_per_point(), 2.0);
    }

    #[test]
    fn graphics_error_reply_is_unsupported() {
        let bytes = b"\x1b_Gi=31;EINVAL:unsupported\x1b\\\x1b[?6c";
        let mut caps = Capabilities::default();
        assert!(parse_replies(bytes, &mut caps));
        assert!(!caps.kitty_graphics);
        assert!(!caps.shm);
    }

    #[test]
    fn no_graphics_only_da() {
        let bytes = b"\x1b[?1;2c";
        let mut caps = Capabilities::default();
        assert!(parse_replies(bytes, &mut caps));
        assert!(!caps.kitty_graphics);
        assert_eq!(caps.kitty_keyboard, None);
        assert_eq!(caps.cell_w, 0);
    }

    #[test]
    fn incomplete_sequence_does_not_finish() {
        let bytes = b"\x1b_Gi=31;OK\x1b\\\x1b[?62;2";
        let mut caps = Capabilities::default();
        assert!(!parse_replies(bytes, &mut caps));
        assert!(caps.kitty_graphics);
    }

    #[test]
    fn stray_bytes_between_replies_are_ignored() {
        let bytes = b"xx\x1b[6;16;8tyy\x1b[?c";
        let mut caps = Capabilities::default();
        assert!(parse_replies(bytes, &mut caps));
        assert_eq!((caps.cell_w, caps.cell_h), (8, 16));
        assert_eq!(caps.pixels_per_point(), 1.0);
    }

    #[test]
    fn osc11_background() {
        let mut caps = Capabilities::default();
        assert!(parse_replies(
            b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\\x1b[?c",
            &mut caps
        ));
        assert_eq!(caps.background, Some([0x1e, 0x1e, 0x2e]));
        let mut caps = Capabilities::default();
        assert!(parse_replies(b"\x1b]11;rgb:ff/ff/ff\x07\x1b[?c", &mut caps));
        assert_eq!(caps.background, Some([0xff, 0xff, 0xff]));
        let mut caps = Capabilities::default();
        assert!(parse_replies(
            b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b[?c",
            &mut caps
        ));
        assert_eq!(caps.background, None, "foreground reply is ignored");
    }

    #[test]
    fn decrqm_states() {
        let mut caps = Capabilities::default();
        parse_replies(b"\x1b[?1016;0$y\x1b[?c", &mut caps);
        assert!(!caps.pixel_mouse, "0 means not recognised");
        parse_replies(b"\x1b[?1016;1$y\x1b[?c", &mut caps);
        assert!(caps.pixel_mouse);
        caps.pixel_mouse = false;
        parse_replies(b"\x1b[?2004;2$y\x1b[?c", &mut caps);
        assert!(!caps.pixel_mouse, "other modes are ignored");
    }

    #[test]
    fn scale_buckets() {
        let mut c = Capabilities::default();
        for (h, want) in [
            (16, 1.0),
            (19, 1.0),
            (20, 1.5),
            (24, 1.5),
            (28, 2.0),
            (34, 2.0),
        ] {
            c.cell_h = h;
            assert_eq!(c.pixels_per_point(), want, "cell_h {h}");
        }
    }
}
