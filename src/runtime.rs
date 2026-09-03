//! The interactive loop and the headless frame renderer. Wires egui, the
//! rasterizer, the framebuffer and the terminal together.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _};

use crate::render::frame::Framebuffer;
use crate::render::raster::{Rasterizer, Target};
use crate::term::{self, kitty, probe};
use crate::ui::app::App;

/// Background color behind everything, matches the egui dark theme panel.
const CLEAR: [u8; 4] = [0x1b, 0x1b, 0x1b, 0xff];
const MAX_TEXTURE_SIDE: usize = 8192;

pub struct Options {
    pub no_shm: bool,
    pub crash: bool,
    pub scale: Option<f32>,
    pub font_size: Option<f32>,
}

/// Minimal quit detection until Phase 2 brings the real parser: `q`,
/// Ctrl+C as a raw byte, and the kitty keyboard encodings `CSI 113 ... u`
/// and `CSI 99 ; 5 u`.
pub fn wants_quit(bytes: &[u8]) -> bool {
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

fn setup_context(ppp: f32, font_size: Option<f32>) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(ppp);
    let body = font_size.unwrap_or(13.0);
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(body - 3.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(body, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(body, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(body + 5.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(body - 0.5, FontFamily::Monospace)),
        ]
        .into();
    });
    ctx
}

fn raw_input(w: u32, h: u32, ppp: f32, time: f64, focused: bool) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(w as f32 / ppp, h as f32 / ppp),
        )),
        max_texture_side: Some(MAX_TEXTURE_SIDE),
        time: Some(time),
        predicted_dt: 1.0 / 60.0,
        focused,
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(ppp);
    input
}

pub struct Timings {
    pub ui: Duration,
    pub tessellate: Duration,
    pub raster: Duration,
}

/// Run one egui pass and rasterize it into `fb`. Returns the repaint delay
/// egui asked for.
fn render_pass(
    ctx: &egui::Context,
    app: &mut App,
    raster: &mut Rasterizer,
    fb: &mut Framebuffer,
    input: egui::RawInput,
    timings: &mut Timings,
) -> Duration {
    let t0 = Instant::now();
    let mut out = ctx.run_ui(input, |ui| app.ui(ui));
    let t1 = Instant::now();
    let shapes = std::mem::take(&mut out.shapes);
    let mut textures = std::mem::take(&mut out.textures_delta);
    let prims = ctx.tessellate(shapes, out.pixels_per_point);
    let t2 = Instant::now();
    raster.apply_set(&textures);
    fb.clear(CLEAR);
    let (w, h) = (fb.width() as usize, fb.height() as usize);
    raster.paint(&mut Target { w, h, rgba: fb.pixels_mut() }, out.pixels_per_point, &prims);
    raster.apply_free(&textures);
    // Dropping an unapplied delta panics in debug builds; we applied it.
    textures.clear();
    let t3 = Instant::now();
    timings.ui = t1 - t0;
    timings.tessellate = t2 - t1;
    timings.raster = t3 - t2;
    out.viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|v| v.repaint_delay)
        .unwrap_or(Duration::MAX)
}

pub fn run_headless(path: &Path, size: (u32, u32), opts: &Options) -> anyhow::Result<i32> {
    let ppp = opts.scale.unwrap_or(1.0);
    let ctx = setup_context(ppp, opts.font_size);
    let mut app = App::new("headless", ppp);
    let mut raster = Rasterizer::new();
    let mut fb = Framebuffer::new(size.0, size.1);
    let mut t = Timings { ui: Duration::ZERO, tessellate: Duration::ZERO, raster: Duration::ZERO };
    // Two passes: the first one loads fonts and settles layout.
    for pass in 0..2 {
        let input = raw_input(size.0, size.1, ppp, pass as f64 / 60.0, true);
        render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
    }
    fb.save_png(path).with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "headless {}x{} scale {ppp}: ui {:.2} ms, tessellate {:.2} ms, raster {:.2} ms -> {}",
        size.0,
        size.1,
        t.ui.as_secs_f64() * 1e3,
        t.tessellate.as_secs_f64() * 1e3,
        t.raster.as_secs_f64() * 1e3,
        path.display()
    );
    Ok(0)
}

pub fn run_interactive(opts: &Options) -> anyhow::Result<i32> {
    if !term::is_tty() {
        bail!("interactive mode needs a terminal on stdin and stdout");
    }
    let raw = term::RawGuard::enter()?;
    let caps = probe::probe(!opts.no_shm, Duration::from_millis(1000)).context("probe failed")?;
    drop(raw);

    if let Some(m) = &caps.multiplexer {
        eprintln!("gitgui: running inside {m}, which does not pass kitty graphics through. Run it directly in Ghostty, cmux or kitty.");
        return Ok(4);
    }
    if !caps.kitty_graphics {
        eprintln!("gitgui: this terminal did not answer the kitty graphics probe. Supported: Ghostty, cmux, kitty, WezTerm.");
        return Ok(3);
    }
    let transport = if caps.shm && !opts.no_shm { kitty::Transport::Shm } else { kitty::Transport::Direct };
    let transport_name = match transport {
        kitty::Transport::Shm => "shm",
        kitty::Transport::Direct => "direct",
    };
    let ppp = opts.scale.unwrap_or_else(|| caps.pixels_per_point());
    let min_interval = match transport {
        kitty::Transport::Shm => Duration::from_millis(16),
        kitty::Transport::Direct => Duration::from_millis(50),
    };

    let session = term::Session::enter()?;
    let mut caps = caps;
    let (w, h) = caps.frame_size();
    let ctx = setup_context(ppp, opts.font_size);
    let mut app = App::new(transport_name, ppp);
    let mut raster = Rasterizer::new();
    let mut fb = Framebuffer::new(w, h);
    let mut enc = kitty::FrameEncoder::new(transport, std::process::id());
    let mut out = Vec::with_capacity(1 << 16);
    let mut inbuf = [0u8; 4096];
    let start = Instant::now();
    let mut t = Timings { ui: Duration::ZERO, tessellate: Duration::ZERO, raster: Duration::ZERO };
    let mut next_deadline = Instant::now();
    let mut last_frame = Instant::now() - min_interval;

    loop {
        if term::quit_requested() {
            break;
        }
        if opts.crash && start.elapsed() > Duration::from_secs(1) {
            panic!("deliberate panic from --crash: the terminal must be restored");
        }
        if term::take_sigwinch() {
            probe::apply_winsize(&mut caps);
            let (nw, nh) = caps.frame_size();
            if (nw, nh) != (fb.width(), fb.height()) {
                fb.resize(nw, nh);
                out.clear();
                kitty::encode_delete_all(&mut out);
                term::write_all(&out)?;
                enc.reset();
                next_deadline = Instant::now();
            }
        }

        let now = Instant::now();
        if now >= next_deadline {
            let since_last = now.duration_since(last_frame);
            if since_last < min_interval {
                next_deadline = last_frame + min_interval;
            } else {
                let input = raw_input(fb.width(), fb.height(), ppp, start.elapsed().as_secs_f64(), true);
                let delay = render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
                let t_send = Instant::now();
                if fb.is_dirty() {
                    out.clear();
                    match transport {
                        kitty::Transport::Shm => {
                            let name = enc.next_shm_name();
                            kitty::Shm::create_and_fill(&name, fb.pixels()).context("shm create")?;
                            enc.encode_frame(&mut out, fb.width(), fb.height(), fb.pixels(), Some(&name));
                        }
                        kitty::Transport::Direct => {
                            enc.encode_frame(&mut out, fb.width(), fb.height(), fb.pixels(), None)
                        }
                    }
                    term::write_all(&out)?;
                    fb.mark_sent();
                }
                let total = t.ui + t.tessellate + t.raster + t_send.elapsed();
                app.frame_ms = total.as_secs_f64() as f32 * 1e3;
                last_frame = Instant::now();
                next_deadline = last_frame + delay.min(Duration::from_secs(3600)).max(min_interval);
            }
        }

        let wait = next_deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(500));
        let n = term::read_timeout(&mut inbuf, wait)?;
        if n > 0 {
            if wants_quit(&inbuf[..n]) {
                break;
            }
            // Any input wakes egui in Phase 2; for now just repaint.
            next_deadline = Instant::now();
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
    fn headless_frame_has_panel_and_background_pixels() {
        let ppp = 1.0;
        let ctx = setup_context(ppp, None);
        let mut app = App::new("test", ppp);
        let mut raster = Rasterizer::new();
        let mut fb = Framebuffer::new(400, 300);
        let mut t = Timings { ui: Duration::ZERO, tessellate: Duration::ZERO, raster: Duration::ZERO };
        for pass in 0..2 {
            let input = raw_input(400, 300, ppp, pass as f64 / 60.0, true);
            render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
        }
        // Something was drawn: not every pixel is the clear color.
        let clear_count = fb.pixels().as_chunks::<4>().0.iter().filter(|p| **p == CLEAR).count();
        assert!(clear_count < 400 * 300, "frame is blank");
        // Text was drawn in the sidebar heading area: some bright pixels.
        let bright = fb.pixels().as_chunks::<4>().0.iter().take(400 * 40).filter(|p| p[0] > 100).count();
        assert!(bright > 20, "no text pixels found in the heading band, got {bright}");
        assert_eq!(fb.pixel(399, 299)[3], 255);
    }
}
