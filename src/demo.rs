//! Phase 0 interactive loop: paint a solid frame with a bouncing square.
//! Replaced by the egui loop in Phase 1.

use std::time::{Duration, Instant};

use anyhow::{bail, Context};

use crate::term::{self, kitty, probe};

const BG: [u8; 4] = [0x1e, 0x1e, 0x2e, 0xff];
const SQUARE: [u8; 4] = [0xf3, 0x8b, 0xa8, 0xff];
const SQUARE_SIZE: u32 = 40;

struct Square {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
}

fn paint(fb: &mut [u8], w: u32, h: u32, sq: &Square) {
    for px in fb.as_chunks_mut::<4>().0 {
        *px = BG;
    }
    let x0 = sq.x.max(0.0) as u32;
    let y0 = sq.y.max(0.0) as u32;
    let x1 = (x0 + SQUARE_SIZE).min(w);
    let y1 = (y0 + SQUARE_SIZE).min(h);
    for y in y0..y1 {
        let row = (y * w) as usize * 4;
        for x in x0..x1 {
            let i = row + x as usize * 4;
            fb[i..i + 4].copy_from_slice(&SQUARE);
        }
    }
}

/// Minimal quit detection for Phase 0: `q`, Ctrl+C as a raw byte, and the
/// kitty keyboard encodings `CSI 113 ... u` and `CSI 99 ; 5 u`.
fn wants_quit(bytes: &[u8]) -> bool {
    if bytes.contains(&0x03) {
        return true;
    }
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'q' => return true,
            0x1b if bytes.get(i + 1) == Some(&b'[') => {
                let body = &bytes[i + 2..];
                let Some(fin) = body.iter().position(|b| (0x40..=0x7e).contains(b)) else {
                    return false;
                };
                if body[fin] == b'u' {
                    let params = std::str::from_utf8(&body[..fin]).unwrap_or("");
                    let mut fields = params.split(';');
                    let key = fields.next().unwrap_or("").split(':').next().unwrap_or("");
                    let mods = fields.next().unwrap_or("1").split(':').next().unwrap_or("1");
                    let event = params.split(';').nth(1).and_then(|m| m.split(':').nth(1)).unwrap_or("1");
                    let released = event == "3";
                    if !released && (key == "113" || (key == "99" && mods == "5")) {
                        return true;
                    }
                }
                i += 2 + fin + 1;
            }
            _ => i += 1,
        }
    }
    false
}

pub fn run(no_shm: bool, crash: bool) -> anyhow::Result<i32> {
    if !term::is_tty() {
        bail!("interactive mode needs a terminal on stdin and stdout");
    }
    let _raw = term::RawGuard::enter()?;
    let caps = probe::probe(!no_shm, Duration::from_millis(1000)).context("probe failed")?;
    drop(_raw);

    if let Some(m) = &caps.multiplexer {
        eprintln!("gitgui: running inside {m}, which does not pass kitty graphics through. Run it directly in Ghostty, cmux or kitty.");
        return Ok(4);
    }
    if !caps.kitty_graphics {
        eprintln!("gitgui: this terminal did not answer the kitty graphics probe. Supported: Ghostty, cmux, kitty, WezTerm.");
        return Ok(3);
    }
    let transport = if caps.shm && !no_shm { kitty::Transport::Shm } else { kitty::Transport::Direct };

    let session = term::Session::enter()?;
    let mut caps = caps;
    let (mut w, mut h) = caps.frame_size();
    let mut fb = vec![0u8; (w * h * 4) as usize];
    let mut enc = kitty::FrameEncoder::new(transport, std::process::id());
    let mut sq = Square { x: 10.0, y: 10.0, dx: 3.0, dy: 2.0 };
    let frame_interval = match transport {
        kitty::Transport::Shm => Duration::from_millis(16),
        kitty::Transport::Direct => Duration::from_millis(50),
    };
    let mut last_sent: Vec<u8> = Vec::new();
    let mut out = Vec::with_capacity(4096);
    let mut inbuf = [0u8; 1024];
    let mut next_frame = Instant::now();
    let started = Instant::now();

    loop {
        if term::quit_requested() {
            break;
        }
        if crash && started.elapsed() > Duration::from_secs(1) {
            panic!("deliberate panic from --crash: the terminal must be restored");
        }
        if term::take_sigwinch() {
            probe::apply_winsize(&mut caps);
            let (nw, nh) = caps.frame_size();
            if (nw, nh) != (w, h) {
                w = nw;
                h = nh;
                fb = vec![0u8; (w * h * 4) as usize];
                last_sent.clear();
                out.clear();
                kitty::encode_delete_all(&mut out);
                term::write_all(&out)?;
                enc.reset();
                sq.x = sq.x.min((w.saturating_sub(SQUARE_SIZE)) as f32);
                sq.y = sq.y.min((h.saturating_sub(SQUARE_SIZE)) as f32);
            }
        }

        let now = Instant::now();
        if now >= next_frame {
            next_frame = now + frame_interval;
            sq.x += sq.dx;
            sq.y += sq.dy;
            let maxx = w.saturating_sub(SQUARE_SIZE) as f32;
            let maxy = h.saturating_sub(SQUARE_SIZE) as f32;
            if sq.x <= 0.0 || sq.x >= maxx {
                sq.dx = -sq.dx;
                sq.x = sq.x.clamp(0.0, maxx);
            }
            if sq.y <= 0.0 || sq.y >= maxy {
                sq.dy = -sq.dy;
                sq.y = sq.y.clamp(0.0, maxy);
            }
            paint(&mut fb, w, h, &sq);
            if fb != last_sent && w > 0 && h > 0 {
                out.clear();
                match transport {
                    kitty::Transport::Shm => {
                        let name = enc.next_shm_name();
                        kitty::Shm::create_and_fill(&name, &fb).context("shm create")?;
                        enc.encode_frame(&mut out, w, h, &fb, Some(&name));
                    }
                    kitty::Transport::Direct => enc.encode_frame(&mut out, w, h, &fb, None),
                }
                term::write_all(&out)?;
                last_sent.clone_from(&fb);
            }
        }

        let wait = next_frame.saturating_duration_since(Instant::now());
        let n = term::read_timeout(&mut inbuf, wait)?;
        if n > 0 && wants_quit(&inbuf[..n]) {
            break;
        }
    }
    drop(session);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_detection() {
        assert!(wants_quit(b"q"));
        assert!(wants_quit(b"\x03"));
        assert!(wants_quit(b"\x1b[113u"));
        assert!(wants_quit(b"\x1b[113;1u"));
        assert!(wants_quit(b"\x1b[113;1:1u"));
        assert!(wants_quit(b"\x1b[113;1:2u"));
        assert!(!wants_quit(b"\x1b[113;1:3u"), "release must not quit");
        assert!(wants_quit(b"\x1b[99;5u"));
        assert!(!wants_quit(b"\x1b[99;1u"));
        assert!(!wants_quit(b"\x1b[<35;100;200M"));
        assert!(!wants_quit(b"abc"));
    }

    #[test]
    fn paint_square_pixels() {
        let (w, h) = (50, 50);
        let mut fb = vec![0u8; (w * h * 4) as usize];
        paint(&mut fb, w, h, &Square { x: 5.0, y: 5.0, dx: 0.0, dy: 0.0 });
        assert_eq!(&fb[0..4], &BG);
        let i = ((10 * w + 10) * 4) as usize;
        assert_eq!(&fb[i..i + 4], &SQUARE);
        let i = ((46 * w + 46) * 4) as usize;
        assert_eq!(&fb[i..i + 4], &BG);
    }
}
