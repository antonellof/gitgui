//! The interactive loop, `--dump-input`, and the headless frame renderer.
//! Wires egui, the rasterizer, the framebuffer and the terminal together.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _};
use base64::Engine as _;

use crate::agent::{self, AgentJob, Server};
use crate::git::ops::{self, Command, Reply};
use crate::git::repo::{GitError, Repo};
use crate::render::frame::Framebuffer;
use crate::render::raster::{Rasterizer, Target};
use crate::term::input::{Event, Key, Parser};
use crate::term::{self, kitty, probe};
use crate::ui::app::App;
use crate::ui::input::Mapper;
use crate::ui::theme::Theme;

const MAX_TEXTURE_SIDE: usize = 8192;
/// A lone ESC becomes the Escape key after this long without more bytes.
const ESC_TIMEOUT: Duration = Duration::from_millis(50);

pub struct Options {
    pub no_shm: bool,
    pub crash: bool,
    pub scale: Option<f32>,
    pub font_size: Option<f32>,
    pub path: PathBuf,
}

/// Everything the main loop waits on arrives through one channel.
enum Msg {
    Input(Vec<u8>),
    Git(Reply),
    Agent(AgentJob),
}

/// UI font size in points that matches the terminal's text: the cell
/// height in logical pixels times a typical font/line-height ratio.
pub fn font_size_for_cell(cell_h_px: u32, ppp: f32) -> f32 {
    if cell_h_px == 0 {
        return 13.0;
    }
    let cell_pt = cell_h_px as f32 / ppp;
    (cell_pt * 0.76).round().clamp(9.0, 24.0)
}

/// The scale is NOT set with `set_pixels_per_point`: egui multiplies its
/// zoom factor by `native_pixels_per_point` from `RawInput`, so setting
/// both would double the scale. Only the raw input carries it.
fn setup_context(font_size: f32, theme: &Theme) -> egui::Context {
    let ctx = egui::Context::default();
    theme.apply(&ctx);
    let body = font_size;
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily, FontId, TextStyle};
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(body - 3.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(body, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(body, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(body + 5.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(body - 0.5, FontFamily::Monospace),
            ),
        ]
        .into();
    });
    ctx
}

fn raw_input(
    w: u32,
    h: u32,
    ppp: f32,
    time: f64,
    focused: bool,
    events: Vec<egui::Event>,
) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(w as f32 / ppp, h as f32 / ppp),
        )),
        max_texture_side: Some(MAX_TEXTURE_SIDE),
        time: Some(time),
        predicted_dt: 1.0 / 60.0,
        focused,
        events,
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(ppp);
    input
}

#[derive(Default)]
pub struct Timings {
    pub ui: Duration,
    pub tessellate: Duration,
    pub raster: Duration,
}

struct PassResult {
    repaint_delay: Duration,
    copy_text: Option<String>,
}

/// Run one egui pass and rasterize it into `fb`.
fn render_pass(
    ctx: &egui::Context,
    app: &mut App,
    raster: &mut Rasterizer,
    fb: &mut Framebuffer,
    input: egui::RawInput,
    timings: &mut Timings,
) -> PassResult {
    let t0 = Instant::now();
    let mut out = ctx.run_ui(input, |ui| app.ui(ui));
    let t1 = Instant::now();
    let shapes = std::mem::take(&mut out.shapes);
    let mut textures = std::mem::take(&mut out.textures_delta);
    let prims = ctx.tessellate(shapes, out.pixels_per_point);
    let t2 = Instant::now();
    raster.apply_set(&textures);
    let bg = app.theme.background;
    fb.clear([bg.r(), bg.g(), bg.b(), 255]);
    let (w, h) = (fb.width() as usize, fb.height() as usize);
    raster.paint(
        &mut Target {
            w,
            h,
            rgba: fb.pixels_mut(),
        },
        out.pixels_per_point,
        &prims,
    );
    raster.apply_free(&textures);
    // Dropping an unapplied delta panics in debug builds; we applied it.
    textures.clear();
    let t3 = Instant::now();
    timings.ui = t1 - t0;
    timings.tessellate = t2 - t1;
    timings.raster = t3 - t2;
    let mut copy_text = None;
    for cmd in out.platform_output.commands.drain(..) {
        if let egui::OutputCommand::CopyText(s) = cmd {
            copy_text = Some(s);
        }
    }
    PassResult {
        repaint_delay: out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX),
        copy_text,
    }
}

/// `OSC 52 ; c ; <base64> ST`: write to the terminal clipboard.
pub fn encode_osc52_copy(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(b"\x1b]52;c;");
    out.extend_from_slice(
        base64::engine::general_purpose::STANDARD
            .encode(text.as_bytes())
            .as_bytes(),
    );
    out.extend_from_slice(b"\x1b\\");
}

pub fn run_headless(path: &Path, size: (u32, u32), opts: &Options) -> anyhow::Result<i32> {
    let ppp = opts.scale.unwrap_or(1.0);
    let theme = Theme::dark();
    let ctx = setup_context(opts.font_size.unwrap_or(13.0), &theme);
    let mut app = App::new(theme, "headless", ppp);
    let mut raster = Rasterizer::new();
    let mut fb = Framebuffer::new(size.0, size.1);
    let mut t = Timings::default();

    // Load the repository synchronously: snapshot, then whatever the app
    // asks for (commit files, first diff) until it is quiet.
    let mut repo = match Repo::open(&opts.path) {
        Ok(r) => r,
        Err(GitError::NotARepository(p)) => {
            eprintln!("gitgui: not a git repository: {}", p.display());
            return Ok(2);
        }
        Err(e) => return Err(e.into()),
    };
    let t_git = Instant::now();
    app.apply(Reply::Snapshot(repo.snapshot(ops::COMMIT_LIMIT)?));
    let git_ms = t_git.elapsed().as_secs_f64() * 1e3;
    for _ in 0..8 {
        let cmds = std::mem::take(&mut app.pending);
        if cmds.is_empty() {
            break;
        }
        for cmd in cmds {
            match cmd {
                Command::LoadDiff(target) => app.apply(Reply::Diff(repo.diff(&target))),
                Command::LoadCommitFiles(oid) => {
                    app.apply(Reply::CommitFiles(oid, repo.commit_files(oid)))
                }
                _ => {}
            }
        }
    }
    // Three passes: fonts load, layout settles, then the final frame.
    for pass in 0..3 {
        let input = raw_input(size.0, size.1, ppp, pass as f64 / 60.0, true, Vec::new());
        render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
    }
    fb.save_png(path)
        .with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "headless {}x{} scale {ppp}: git {git_ms:.1} ms ({} commits), ui {:.2} ms, tessellate {:.2} ms, raster {:.2} ms -> {}",
        size.0,
        size.1,
        app.snapshot.commits.len(),
        t.ui.as_secs_f64() * 1e3,
        t.tessellate.as_secs_f64() * 1e3,
        t.raster.as_secs_f64() * 1e3,
        path.display()
    );
    Ok(0)
}

/// Spawn the stdin reader thread. It blocks in `poll` + `read` and ships
/// raw byte chunks over the channel until stdin closes.
fn spawn_stdin_thread<T: Send + 'static>(
    tx: mpsc::Sender<T>,
    wrap: impl Fn(Vec<u8>) -> T + Send + 'static,
) {
    std::thread::Builder::new()
        .name("stdin".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match term::read_timeout(&mut buf, Duration::from_secs(3600)) {
                    Ok(0) => continue,
                    Ok(n) => {
                        if tx.send(wrap(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("spawn stdin thread");
}

struct Probed {
    caps: probe::Capabilities,
    transport: kitty::Transport,
}

fn probe_or_exit(no_shm: bool) -> anyhow::Result<Result<Probed, i32>> {
    if !term::is_tty() {
        bail!("interactive mode needs a terminal on stdin and stdout");
    }
    let raw = term::RawGuard::enter()?;
    let caps = probe::probe(!no_shm, Duration::from_millis(1000)).context("probe failed")?;
    drop(raw);
    if let Some(m) = &caps.multiplexer {
        eprintln!("gitgui: running inside {m}, which does not pass kitty graphics through. Run it directly in Ghostty, cmux or kitty.");
        return Ok(Err(4));
    }
    if !caps.kitty_graphics {
        eprintln!("gitgui: this terminal did not answer the kitty graphics probe. Supported: Ghostty, cmux, kitty, WezTerm.");
        return Ok(Err(3));
    }
    let transport = if caps.shm && !no_shm {
        kitty::Transport::Shm
    } else {
        kitty::Transport::Direct
    };
    Ok(Ok(Probed { caps, transport }))
}

/// Print decoded input events until Ctrl+C.
pub fn run_dump_input() -> anyhow::Result<i32> {
    if !term::is_tty() {
        bail!("--dump-input needs a terminal");
    }
    let caps = {
        let _raw = term::RawGuard::enter()?;
        probe::probe(false, Duration::from_millis(1000))?
    };
    let session = term::Session::enter()?;
    let mut parser = Parser::new(caps.pixel_mouse, caps.cell_w, caps.cell_h);
    let mut line = format!(
        "gitgui --dump-input: kitty keyboard {:?}, pixel mouse {}, cell {}x{}. Ctrl+C exits.\r\n",
        caps.kitty_keyboard, caps.pixel_mouse, caps.cell_w, caps.cell_h
    )
    .into_bytes();
    term::write_all(&line)?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    spawn_stdin_thread(tx, |b| b);
    let mut last_byte = Instant::now();
    loop {
        if term::quit_requested() {
            break;
        }
        let events = match rx.recv_timeout(ESC_TIMEOUT) {
            Ok(bytes) => {
                last_byte = Instant::now();
                line.clear();
                line.extend_from_slice(b"raw: ");
                for b in &bytes {
                    line.extend_from_slice(format!("{b:02x} ").as_bytes());
                }
                line.extend_from_slice(b"\r\n");
                term::write_all(&line)?;
                parser.feed(&bytes)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if parser.has_pending() && last_byte.elapsed() >= ESC_TIMEOUT {
                    parser.flush()
                } else {
                    continue;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut quit = false;
        for ev in &events {
            line.clear();
            line.extend_from_slice(format!("  {ev:?}\r\n").as_bytes());
            term::write_all(&line)?;
            if matches!(ev, Event::Key { key: Key::Char('c'), mods, pressed: true, .. } if mods.ctrl)
            {
                quit = true;
            }
        }
        if quit {
            break;
        }
    }
    drop(session);
    Ok(0)
}

pub fn run_interactive(opts: &Options) -> anyhow::Result<i32> {
    let Probed {
        mut caps,
        transport,
    } = match probe_or_exit(opts.no_shm)? {
        Ok(p) => p,
        Err(code) => return Ok(code),
    };
    let transport_name = match transport {
        kitty::Transport::Shm => "shm",
        kitty::Transport::Direct => "direct",
    };
    let mut ppp = opts.scale.unwrap_or_else(|| caps.pixels_per_point());
    let min_interval = match transport {
        kitty::Transport::Shm => Duration::from_millis(16),
        kitty::Transport::Direct => Duration::from_millis(50),
    };

    // Start git before touching the screen so a bad path exits cleanly.
    let (tx, rx) = mpsc::channel::<Msg>();
    let git_tx = tx.clone();
    let (agent_job_tx, agent_job_rx) = mpsc::channel::<AgentJob>();
    let agent_bridge = tx.clone();
    std::thread::Builder::new()
        .name("agent-bridge".into())
        .spawn(move || {
            while let Ok(job) = agent_job_rx.recv() {
                if agent_bridge.send(Msg::Agent(job)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn agent bridge");
    let _agent = Server::bind(&opts.path, agent_job_tx).context("agent socket")?;
    let worker = match ops::spawn(opts.path.clone(), move |r| {
        let _ = git_tx.send(Msg::Git(r));
    }) {
        Ok(w) => w,
        Err(GitError::NotARepository(p)) => {
            eprintln!("gitgui: not a git repository: {}", p.display());
            return Ok(2);
        }
        Err(e) => return Err(e.into()),
    };

    let session = term::Session::enter()?;
    let (w, h) = caps.frame_size();
    let theme = Theme::from_background(caps.background);
    let font_size = opts
        .font_size
        .unwrap_or_else(|| font_size_for_cell(caps.cell_h, ppp));
    let ctx = setup_context(font_size, &theme);
    let mut app = App::new(theme, transport_name, ppp);
    let mut raster = Rasterizer::new();
    let mut fb = Framebuffer::new(w, h);
    let mut enc = kitty::FrameEncoder::new(transport, std::process::id());
    let mut parser = Parser::new(caps.pixel_mouse, caps.cell_w, caps.cell_h);
    let mut mapper = Mapper::new(ppp);
    spawn_stdin_thread(tx, Msg::Input);

    let mut out = Vec::with_capacity(1 << 16);
    let mut pending: Vec<egui::Event> = Vec::new();
    let mut focused = true;
    let start = Instant::now();
    let mut t = Timings::default();
    let mut next_deadline = Instant::now();
    let mut last_frame = Instant::now() - min_interval;
    let mut last_byte = Instant::now();
    let mut resize_needed = false;
    let mut screenshot: Option<std::path::PathBuf> = None;
    let mut screenshot_reply: Option<mpsc::Sender<String>> = None;

    loop {
        if term::quit_requested() {
            break;
        }
        if opts.crash && start.elapsed() > Duration::from_secs(1) {
            panic!("deliberate panic from --crash: the terminal must be restored");
        }
        if term::take_sigwinch() {
            // Grid from the ioctl right away; the cell size may also have
            // changed (font zoom, another display), so ask the terminal and
            // apply the reply when it arrives through the input stream.
            probe::apply_winsize(&mut caps);
            term::write_all(b"\x1b[16t\x1b[14t\x1b[18t")?;
            resize_needed = true;
        }
        if resize_needed {
            resize_needed = false;
            let (nw, nh) = caps.frame_size();
            if opts.scale.is_none() {
                let new_ppp = caps.pixels_per_point();
                if new_ppp != ppp {
                    ppp = new_ppp;
                    mapper.set_ppp(ppp);
                    app.scale = ppp;
                    if opts.font_size.is_none() {
                        let fs = font_size_for_cell(caps.cell_h, ppp);
                        ctx.all_styles_mut(|style| {
                            use egui::{FontFamily, FontId, TextStyle};
                            style.text_styles = [
                                (
                                    TextStyle::Small,
                                    FontId::new(fs - 3.0, FontFamily::Proportional),
                                ),
                                (TextStyle::Body, FontId::new(fs, FontFamily::Proportional)),
                                (TextStyle::Button, FontId::new(fs, FontFamily::Proportional)),
                                (
                                    TextStyle::Heading,
                                    FontId::new(fs + 5.0, FontFamily::Proportional),
                                ),
                                (
                                    TextStyle::Monospace,
                                    FontId::new(fs - 0.5, FontFamily::Monospace),
                                ),
                            ]
                            .into();
                        });
                    }
                }
            }
            if (nw, nh) != (fb.width(), fb.height()) {
                fb.resize(nw, nh);
                out.clear();
                kitty::encode_delete_all(&mut out);
                term::write_all(&out)?;
                enc.reset();
            }
            parser.cell_w = caps.cell_w.max(1);
            parser.cell_h = caps.cell_h.max(1);
            next_deadline = Instant::now();
        }

        let now = Instant::now();
        if now >= next_deadline {
            let since_last = now.duration_since(last_frame);
            if since_last < min_interval {
                next_deadline = last_frame + min_interval;
            } else {
                mapper.flush(&mut pending);
                let events = std::mem::take(&mut pending);
                let input = raw_input(
                    fb.width(),
                    fb.height(),
                    ppp,
                    start.elapsed().as_secs_f64(),
                    focused,
                    events,
                );
                let pass = render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
                let t_send = Instant::now();
                out.clear();
                if let Some(text) = pass.copy_text {
                    encode_osc52_copy(&mut out, &text);
                }
                if fb.is_dirty() {
                    match transport {
                        kitty::Transport::Shm => {
                            let name = enc.next_shm_name();
                            kitty::Shm::create_and_fill(&name, fb.pixels())
                                .context("shm create")?;
                            enc.encode_frame(
                                &mut out,
                                fb.width(),
                                fb.height(),
                                caps.cols,
                                caps.rows,
                                fb.pixels(),
                                Some(&name),
                            );
                        }
                        kitty::Transport::Direct => enc.encode_frame(
                            &mut out,
                            fb.width(),
                            fb.height(),
                            caps.cols,
                            caps.rows,
                            fb.pixels(),
                            None,
                        ),
                    }
                    fb.mark_sent();
                }
                if let Some(path) = screenshot.take() {
                    let resp = match fb.save_png(&path) {
                        Ok(()) => agent::ok(serde_json::json!({ "path": path })),
                        Err(e) => agent::err(format!("screenshot: {e:#}")),
                    };
                    if let Some(reply) = screenshot_reply.take() {
                        let _ = reply.send(resp);
                    }
                }
                if !out.is_empty() {
                    term::write_all(&out)?;
                }
                let total = t.ui + t.tessellate + t.raster + t_send.elapsed();
                app.frame_ms = total.as_secs_f64() as f32 * 1e3;
                for cmd in app.pending.drain(..) {
                    let _ = worker.tx.send(cmd);
                }
                last_frame = Instant::now();
                next_deadline = last_frame
                    + pass
                        .repaint_delay
                        .min(Duration::from_secs(3600))
                        .max(min_interval);
            }
        }

        // Wait for input, the repaint deadline, or the escape timeout. The
        // cap keeps signal flags (SIGTERM, SIGHUP, SIGWINCH) honored within
        // half a second even when nothing else happens.
        let mut wait = next_deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(500));
        if parser.has_pending() {
            wait = wait.min(ESC_TIMEOUT.saturating_sub(last_byte.elapsed()));
        }
        let events = match rx.recv_timeout(wait) {
            Ok(Msg::Input(bytes)) => {
                last_byte = Instant::now();
                parser.feed(&bytes)
            }
            Ok(Msg::Git(reply)) => {
                app.apply(reply);
                // Drain anything else that is already queued.
                while let Ok(Msg::Git(r)) = rx.try_recv() {
                    app.apply(r);
                }
                for cmd in app.pending.drain(..) {
                    let _ = worker.tx.send(cmd);
                }
                next_deadline = Instant::now();
                continue;
            }
            Ok(Msg::Agent(job)) => {
                if matches!(job.request, agent::AgentCmd::Screenshot { .. }) {
                    screenshot_reply = Some(job.reply);
                    let _ = agent::handle_in_app(&mut app, job.request, &mut screenshot);
                    next_deadline = Instant::now();
                } else {
                    let resp = agent::handle_in_app(&mut app, job.request, &mut screenshot);
                    let _ = job.reply.send(resp);
                }
                for cmd in app.pending.drain(..) {
                    let _ = worker.tx.send(cmd);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if parser.has_pending() && last_byte.elapsed() >= ESC_TIMEOUT {
                    parser.flush()
                } else {
                    Vec::new()
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break, // stdin closed: the terminal is gone
        };
        if events.is_empty() {
            continue;
        }
        let mut quit = false;
        for ev in &events {
            match ev {
                Event::Key {
                    key: Key::Char('c'),
                    mods,
                    pressed: true,
                    ..
                } if mods.ctrl => quit = true,
                Event::Key {
                    key: Key::Char('q'),
                    mods,
                    pressed: true,
                    ..
                } if *mods == crate::term::input::Mods::NONE
                    && !ctx.egui_wants_keyboard_input() =>
                {
                    quit = true
                }
                Event::Focus(f) => {
                    focused = *f;
                    let _ = worker.tx.send(Command::Focus(*f));
                }
                Event::Unknown(bytes) if bytes.ends_with(b"t") => {
                    // Size replies requested after SIGWINCH.
                    let before = (caps.cell_w, caps.cell_h, caps.cols, caps.rows);
                    probe::parse_replies(bytes, &mut caps);
                    if (caps.cell_w, caps.cell_h, caps.cols, caps.rows) != before {
                        resize_needed = true;
                    }
                }
                _ => {}
            }
            mapper.map(ev, &mut pending);
        }
        if quit {
            break;
        }
        next_deadline = Instant::now();
    }
    let _ = worker.tx.send(Command::Quit);
    drop(session);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_size_follows_cell_height() {
        assert_eq!(font_size_for_cell(34, 2.0), 13.0);
        assert_eq!(font_size_for_cell(17, 1.0), 13.0);
        assert_eq!(font_size_for_cell(24, 1.5), 12.0);
        assert_eq!(font_size_for_cell(0, 2.0), 13.0);
        assert_eq!(font_size_for_cell(80, 1.0), 24.0, "clamped");
    }

    #[test]
    fn scale_comes_only_from_native_pixels_per_point() {
        // Regression: setting pixels_per_point on the context and passing
        // native_pixels_per_point doubled the scale on screen.
        let theme = Theme::dark();
        let ctx = setup_context(13.0, &theme);
        let mut app = App::new(theme, "test", 2.0);
        let mut raster = Rasterizer::new();
        let mut fb = Framebuffer::new(200, 100);
        let mut t = Timings::default();
        for pass in 0..3 {
            let input = raw_input(200, 100, 2.0, pass as f64 / 60.0, true, Vec::new());
            render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
        }
        assert_eq!(ctx.pixels_per_point(), 2.0);
        assert_eq!(ctx.content_rect().width(), 100.0);
    }

    /// Drive the real app with a real repository and synthetic terminal
    /// events, the same path the interactive loop takes.
    struct Harness {
        ctx: egui::Context,
        app: App,
        repo: Repo,
        raster: Rasterizer,
        fb: Framebuffer,
        mapper: Mapper,
        t: Timings,
        time: f64,
    }

    impl Harness {
        fn new(dir: &std::path::Path) -> Self {
            let theme = Theme::dark();
            let ctx = setup_context(13.0, &theme);
            let mut app = App::new(theme, "test", 1.0);
            let mut repo = Repo::open(dir).unwrap();
            app.apply(Reply::Snapshot(repo.snapshot(100).unwrap()));
            let mut h = Harness {
                ctx,
                app,
                repo,
                raster: Rasterizer::new(),
                fb: Framebuffer::new(900, 700),
                mapper: Mapper::new(1.0),
                t: Timings::default(),
                time: 0.0,
            };
            h.settle();
            h
        }

        /// Run pending git commands synchronously and render until quiet.
        fn settle(&mut self) {
            for _ in 0..6 {
                let cmds = std::mem::take(&mut self.app.pending);
                for cmd in cmds {
                    match cmd {
                        Command::LoadDiff(target) => {
                            self.app.apply(Reply::Diff(self.repo.diff(&target)))
                        }
                        Command::LoadCommitFiles(oid) => self
                            .app
                            .apply(Reply::CommitFiles(oid, self.repo.commit_files(oid))),
                        Command::Stage(p) => {
                            self.repo.stage(&p).unwrap();
                            self.finish("stage");
                        }
                        Command::StageAll => {
                            self.repo.stage_all().unwrap();
                            self.finish("stage");
                        }
                        Command::Unstage(p) => {
                            self.repo.unstage(&p).unwrap();
                            self.finish("unstage");
                        }
                        Command::Discard(p) => {
                            self.repo.discard(&p).unwrap();
                            self.finish("discard");
                        }
                        Command::StageHunk { path, hunk_index } => {
                            self.repo.stage_hunk(&path, hunk_index).unwrap();
                            self.finish("stage");
                        }
                        Command::Commit { message, amend } => {
                            self.repo.commit(&message, amend).unwrap();
                            self.finish("commit");
                        }
                        other => panic!("unexpected command {other:?}"),
                    }
                }
                self.frame(Vec::new());
            }
        }

        fn finish(&mut self, label: &'static str) {
            self.app.apply(Reply::Op {
                label,
                result: Ok("ok".into()),
            });
            self.app
                .apply(Reply::Snapshot(self.repo.snapshot(100).unwrap()));
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let input = raw_input(900, 700, 1.0, self.time, true, events);
            render_pass(
                &self.ctx,
                &mut self.app,
                &mut self.raster,
                &mut self.fb,
                input,
                &mut self.t,
            );
        }

        fn key(&mut self, bytes: &[u8]) {
            let mut parser = Parser::new(true, 1, 1);
            let mut events = Vec::new();
            for ev in parser.feed(bytes).iter().chain(parser.flush().iter()) {
                self.mapper.map(ev, &mut events);
            }
            self.frame(events);
            self.frame(Vec::new());
        }

        fn click(&mut self, pos: egui::Pos2) {
            use crate::term::input::{Mods, MouseButton};
            let (x, y) = (pos.x as i32, pos.y as i32);
            let mut events = Vec::new();
            self.mapper.map(
                &Event::MouseButton {
                    button: MouseButton::Left,
                    pressed: true,
                    x,
                    y,
                    mods: Mods::NONE,
                },
                &mut events,
            );
            self.frame(events);
            let mut events = Vec::new();
            self.mapper.map(
                &Event::MouseButton {
                    button: MouseButton::Left,
                    pressed: false,
                    x,
                    y,
                    mods: Mods::NONE,
                },
                &mut events,
            );
            self.frame(events);
            self.frame(Vec::new());
        }
    }

    #[test]
    fn keyboard_stage_commit_workflow() {
        use crate::git::repo::testutil::TempRepo;
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.write("a.txt", "one\ntwo\n");
        t.write("b.txt", "new\n");
        let mut h = Harness::new(&t.dir);
        assert_eq!(h.app.selection, crate::ui::app::Selection::WorkingTree);
        assert_eq!(h.app.snapshot.unstaged.len(), 2);

        // `s` stages the selected (first unstaged) file.
        h.key(b"s");
        h.settle();
        assert_eq!(h.app.snapshot.staged.len(), 1);
        assert_eq!(h.app.snapshot.staged[0].path, "a.txt");
        // `a` stages everything.
        h.key(b"a");
        h.settle();
        assert_eq!(h.app.snapshot.staged.len(), 2);
        assert!(h.app.snapshot.unstaged.is_empty());
        // `u` on the staged file that is now selected unstages it, `A` unstages all.
        h.key(b"u");
        h.settle();
        assert_eq!(h.app.snapshot.staged.len(), 1);
        h.key(b"a");
        h.settle();
        // `c` focuses the commit box, typed text lands there, `q` does not quit
        // (it is text now), Ctrl+Enter commits.
        h.key(b"c");
        assert!(h.ctx.egui_wants_keyboard_input());
        h.key(b"fix things");
        h.key(b"q");
        assert_eq!(h.app.commit_msg, "fix thingsq");
        h.key(b"\x1b[13;5u");
        assert!(h.app.pending.iter().any(
            |c| matches!(c, Command::Commit { message, amend: false } if message == "fix thingsq")
        ));
        h.settle();
        assert_eq!(h.app.snapshot.commits.len(), 2);
        assert_eq!(h.app.snapshot.commits[0].summary, "fix thingsq");
        assert!(!h.app.snapshot.is_dirty());
        assert!(h.app.commit_msg.is_empty(), "message cleared after commit");
        // Escape leaves the text field so single-key bindings work again.
        h.key(b"\x1b");
        assert!(!h.ctx.egui_wants_keyboard_input());
    }

    #[test]
    fn commit_button_and_discard_modal() {
        use crate::git::repo::testutil::TempRepo;
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.write("a.txt", "changed\n");
        let mut h = Harness::new(&t.dir);
        h.key(b"s");
        h.settle();
        h.app.commit_msg = "via button".into();
        h.frame(Vec::new());
        let rect = h.app.commit_button_rect.expect("commit button laid out");
        h.click(rect.center());
        assert!(h
            .app
            .pending
            .iter()
            .any(|c| matches!(c, Command::Commit { .. })));
        h.settle();
        assert_eq!(h.app.snapshot.commits[0].summary, "via button");

        // Discard: modal asks first, Escape cancels, Enter confirms.
        t.write("a.txt", "junk\n");
        h.app.apply(Reply::Snapshot(h.repo.snapshot(100).unwrap()));
        h.settle();
        assert_eq!(h.app.snapshot.unstaged.len(), 1);
        h.app.modal = Some(crate::ui::app::Modal::Discard(vec!["a.txt".into()]));
        h.frame(Vec::new());
        h.key(b"\x1b");
        assert!(h.app.modal.is_none(), "escape closes the dialog");
        assert!(h.app.pending.is_empty());
        h.app.modal = Some(crate::ui::app::Modal::Discard(vec!["a.txt".into()]));
        h.frame(Vec::new());
        h.key(b"\r");
        assert!(h
            .app
            .pending
            .iter()
            .any(|c| matches!(c, Command::Discard(p) if p == &["a.txt".to_string()])));
        h.settle();
        assert!(!h.app.snapshot.is_dirty());
        assert_eq!(
            std::fs::read_to_string(t.dir.join("a.txt")).unwrap(),
            "changed\n"
        );
    }

    #[test]
    fn osc52_bytes() {
        let mut out = Vec::new();
        encode_osc52_copy(&mut out, "hi");
        assert_eq!(out, b"\x1b]52;c;aGk=\x1b\\");
    }

    #[test]
    fn headless_frame_has_panel_and_background_pixels() {
        let ppp = 1.0;
        let theme = Theme::dark();
        let ctx = setup_context(13.0, &theme);
        let mut app = App::new(theme, "test", ppp);
        let mut raster = Rasterizer::new();
        let mut fb = Framebuffer::new(400, 300);
        let mut t = Timings::default();
        for pass in 0..2 {
            let input = raw_input(400, 300, ppp, pass as f64 / 60.0, true, Vec::new());
            render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut t);
        }
        let bg = app.theme.background;
        let clear = [bg.r(), bg.g(), bg.b(), 255];
        let clear_count = fb
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == clear)
            .count();
        assert!(clear_count < 400 * 300, "frame is blank");
        let bright = fb
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .take(400 * 40)
            .filter(|p| p[0] > 100)
            .count();
        assert!(
            bright > 20,
            "no text pixels found in the heading band, got {bright}"
        );
        assert_eq!(fb.pixel(399, 299)[3], 255);
    }

    #[test]
    fn headless_renders_a_real_repo_and_selects_first_commit_file() {
        use crate::git::repo::testutil::TempRepo;
        let t = TempRepo::new();
        t.commit_file("hello.txt", "hello\nworld\n", "first commit");
        let ppp = 1.0;
        let theme = Theme::dark();
        let ctx = setup_context(13.0, &theme);
        let mut app = App::new(theme, "test", ppp);
        let mut repo = Repo::open(&t.dir).unwrap();
        app.apply(Reply::Snapshot(repo.snapshot(100).unwrap()));
        // Clean tree: first commit is selected and its files are requested.
        assert_eq!(app.selection, crate::ui::app::Selection::Commit(0));
        let cmds = std::mem::take(&mut app.pending);
        assert!(matches!(cmds[0], Command::LoadCommitFiles(_)));
        let Command::LoadCommitFiles(oid) = cmds[0].clone() else {
            unreachable!()
        };
        app.apply(Reply::CommitFiles(oid, repo.commit_files(oid)));
        let cmds = std::mem::take(&mut app.pending);
        assert!(
            matches!(&cmds[0], Command::LoadDiff(crate::git::repo::DiffTarget::Commit(_, p)) if p == "hello.txt")
        );
        let Command::LoadDiff(target) = cmds[0].clone() else {
            unreachable!()
        };
        app.apply(Reply::Diff(repo.diff(&target)));
        assert_eq!(app.diff.as_ref().unwrap().hunks[0].lines.len(), 2);
        let mut raster = Rasterizer::new();
        let mut fb = Framebuffer::new(800, 500);
        let mut tm = Timings::default();
        for pass in 0..3 {
            let input = raw_input(800, 500, ppp, pass as f64 / 60.0, true, Vec::new());
            render_pass(&ctx, &mut app, &mut raster, &mut fb, input, &mut tm);
        }
        // The diff pane paints added-line backgrounds somewhere in the frame.
        let add = app.theme.add_bg;
        let add_px = [add.r(), add.g(), add.b(), 255];
        let n = fb
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == add_px)
            .count();
        assert!(n > 100, "expected added-line background pixels, got {n}");
    }
}
