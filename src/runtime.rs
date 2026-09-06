//! The interactive loop, `--dump-input`, and the headless frame renderer.
//! Wires the iced shell, the framebuffer and the terminal together.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _};
use base64::Engine as _;
use iced_core::window::RedrawRequest;

use crate::agent::{self, AgentJob, Server};
use crate::git::ops::{self, Command, Reply};
use crate::git::repo::{GitError, Repo};
use crate::render::frame::Framebuffer;
use crate::shell::Shell;
use crate::term::input::{Event, Key, Parser};
use crate::term::{self, kitty, probe};
use crate::ui::app::App;
use crate::ui::theme::Theme;

const ESC_TIMEOUT: Duration = Duration::from_millis(50);

pub struct Options {
    pub no_shm: bool,
    pub crash: bool,
    pub scale: Option<f32>,
    pub font_size: Option<f32>,
    pub editor: Option<String>,
    pub open: Option<String>,
    pub path: PathBuf,
}

enum Msg {
    Input(Vec<u8>),
    Git(Reply),
    Agent(AgentJob),
}

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
    let mut shell = Shell::new(opts.font_size.unwrap_or(13.0), ppp, size.0, size.1, theme.iced());
    let mut app = App::new(theme, "headless", ppp, opts.path.to_path_buf());
    app.editor_cmd = opts.editor.clone();
    app.open_on_start = opts.open.clone();
    let mut fb = Framebuffer::new(size.0, size.1);

    // Load the repository synchronously: snapshot, then whatever the app
    // asks for (commit files, first diff) until it is quiet.
    let mut repo = match Repo::open(&opts.path) {
        Ok(r) => Some(r),
        Err(GitError::NotARepository(p)) => {
            app.no_repo = true;
            app.repo_path = p;
            None
        }
        Err(e) => return Err(e.into()),
    };
    let mut git_ms = 0.0;
    if let Some(repo) = repo.as_mut() {
        let t_git = Instant::now();
        app.apply(Reply::Snapshot(repo.snapshot(ops::COMMIT_LIMIT)?));
        git_ms = t_git.elapsed().as_secs_f64() * 1e3;
        settle(&mut app, repo);
    }
    // Three passes: fonts load, layout settles, then the final frame.
    let mut ui_ms = 0.0;
    for _ in 0..3 {
        let t0 = Instant::now();
        shell.frame(&mut app, &mut fb);
        ui_ms = t0.elapsed().as_secs_f64() * 1e3;
        if let Some(repo) = repo.as_mut() {
            settle(&mut app, repo);
        }
    }
    fb.save_png(path)
        .with_context(|| format!("writing {}", path.display()))?;
    eprintln!(
        "headless {}x{} scale {ppp}: git {git_ms:.1} ms ({} commits), frame {ui_ms:.2} ms -> {}",
        size.0,
        size.1,
        app.snapshot.commits.len(),
        path.display()
    );
    Ok(0)
}

/// Run the app's pending read commands synchronously against `repo`.
pub fn settle(app: &mut App, repo: &mut Repo) {
    for _ in 0..8 {
        let cmds = std::mem::take(&mut app.pending);
        if cmds.is_empty() {
            break;
        }
        for cmd in cmds {
            match cmd {
                Command::LoadDiff(target) => {
                    app.apply(Reply::Diff(repo.diff(&target, app.diff_opts)))
                }
                Command::LoadCommitFiles(oid) => app.apply(Reply::CommitFiles(oid, repo.commit_files(oid))),
                Command::ListDir(dir) => {
                    let r = repo.list_dir(&dir);
                    app.apply(Reply::DirEntries(dir, r))
                }
                _ => {}
            }
        }
    }
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
    let worker = ops::spawn(opts.path.clone(), move |r| {
        let _ = git_tx.send(Msg::Git(r));
    });

    let session = term::Session::enter()?;
    let (w, h) = caps.frame_size();
    let theme = Theme::from_background(caps.background);
    let mut font_size = opts
        .font_size
        .unwrap_or_else(|| font_size_for_cell(caps.cell_h, ppp));
    let mut shell = Shell::new(font_size, ppp, w, h, theme.iced());
    let mut app = App::new(theme, transport_name, ppp, opts.path.clone());
    app.editor_cmd = opts.editor.clone();
    app.open_on_start = opts.open.clone();
    let mut fb = Framebuffer::new(w, h);
    let mut enc = kitty::FrameEncoder::new(transport, std::process::id());
    let mut parser = Parser::new(caps.pixel_mouse, caps.cell_w, caps.cell_h);
    spawn_stdin_thread(tx, Msg::Input);

    let mut out = Vec::with_capacity(1 << 16);
    let start = Instant::now();
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
                    app.scale = ppp;
                    if opts.font_size.is_none() {
                        let fs = font_size_for_cell(caps.cell_h, ppp);
                        if fs != font_size {
                            font_size = fs;
                            shell = Shell::new(font_size, ppp, nw, nh, app.theme.iced());
                        }
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
            shell.resize(nw, nh, ppp);
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
                let t0 = Instant::now();
                if let Some(p) = shell.cursor() {
                    app.cursor = p;
                }
                app.modifiers = shell.modifiers();
                let pass = shell.frame(&mut app, &mut fb);
                out.clear();
                for text in pass.copy.iter().chain(app.pending_copy.iter()) {
                    encode_osc52_copy(&mut out, text);
                }
                app.pending_copy.clear();
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
                app.frame_ms = t0.elapsed().as_secs_f64() as f32 * 1e3;
                for cmd in app.pending.drain(..) {
                    let _ = worker.tx.send(cmd);
                }
                if app.quit {
                    break;
                }
                last_frame = Instant::now();
                let delay = match pass.redraw {
                    RedrawRequest::NextFrame => Duration::ZERO,
                    RedrawRequest::At(at) => at.saturating_duration_since(last_frame),
                    RedrawRequest::Wait => Duration::from_secs(3600),
                };
                next_deadline = last_frame + delay.max(min_interval);
            }
        }

        // Wait for input, the repaint deadline, or the escape timeout. The
        // cap keeps signal flags honored within half a second.
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
                next_deadline = Instant::now();
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if parser.has_pending() && last_byte.elapsed() >= ESC_TIMEOUT {
                    parser.flush()
                } else {
                    Vec::new()
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
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
                Event::Focus(f) => {
                    let _ = worker.tx.send(Command::Focus(*f));
                    shell.push(ev);
                }
                Event::Unknown(bytes) if bytes.ends_with(b"t") => {
                    let before = (caps.cell_w, caps.cell_h, caps.cols, caps.rows);
                    probe::parse_replies(bytes, &mut caps);
                    if (caps.cell_w, caps.cell_h, caps.cols, caps.rows) != before {
                        resize_needed = true;
                    }
                }
                _ => shell.push(ev),
            }
        }
        if quit {
            break;
        }
        next_deadline = Instant::now();
    }
    drop(session);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_size_tracks_cell_height() {
        assert_eq!(font_size_for_cell(0, 2.0), 13.0);
        assert_eq!(font_size_for_cell(34, 2.0), 13.0);
        assert_eq!(font_size_for_cell(17, 1.0), 13.0);
        assert_eq!(font_size_for_cell(60, 1.0), 24.0);
    }
}
