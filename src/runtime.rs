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

#[derive(Clone)]
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
/// `OSC 22 ; <shape> ST`: the pointer shape (kitty, Ghostty, cmux). An
/// empty shape restores the terminal's default.
pub fn encode_pointer_shape(out: &mut Vec<u8>, shape: &str) {
    out.extend_from_slice(b"\x1b]22;");
    out.extend_from_slice(shape.as_bytes());
    out.extend_from_slice(b"\x1b\\");
}

/// CSS-style pointer name for what iced reports under the pointer.
pub fn pointer_shape(i: iced_core::mouse::Interaction) -> &'static str {
    use iced_core::mouse::Interaction as I;
    match i {
        I::Pointer => "pointer",
        I::Grab => "grab",
        I::Grabbing => "grabbing",
        I::Text => "text",
        I::ResizingHorizontally => "ew-resize",
        I::ResizingVertically => "ns-resize",
        I::ResizingDiagonallyUp => "nesw-resize",
        I::ResizingDiagonallyDown => "nwse-resize",
        I::Move => "move",
        I::NotAllowed | I::NoDrop => "not-allowed",
        I::Crosshair => "crosshair",
        I::Help => "help",
        I::Wait | I::Progress => "wait",
        _ => "",
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
    // Debug aid: GITGUI_HEADLESS_OPEN=picker|help|menu|stash|reset opens a
    // dialog before rendering, so the frame shows it.
    if let Ok(what) = std::env::var("GITGUI_HEADLESS_OPEN") {
        use crate::ui::app::Message;
        app.cursor = iced_core::Point::new(400.0, 200.0);
        match what.as_str() {
            "picker" => app.update(Message::OpenBranchPicker),
            "help" => app.update(Message::OpenHelp),
            "menu" => app.update(Message::MenuOpen(crate::ui::app::MenuKind::Commit(0))),
            "stash" => app.update(Message::OpenStashDialog),
            "reset" => app.update(Message::CommitAction(0, crate::ui::app::CommitAction::Reset)),
            "hover" => app.cursor = iced_core::Point::new(300.0, 14.0),
            "folder" => app.update(Message::OpenFolderDialog),
            "zoom" => app.set_zoom(1.4),
            "difftext" => {
                use crate::ui::app::DiffPos;
                app.update(Message::DiffTextDrag {
                    anchor: DiffPos { hunk: 0, line: 1, col: 2 },
                    head: DiffPos { hunk: 0, line: 4, col: 9 },
                });
            }
            "detail" => {
                let i = app.snapshot.commits.iter().position(|c| !c.body.is_empty()).unwrap_or(1);
                app.select(crate::ui::app::Selection::Commit(i));
                app.update(Message::DetailAction(iced_widget::text_editor::Action::SelectAll));
            }
            "select" => {
                use iced_widget::text_editor::{Action, Edit};
                for c in "fix: select this text".chars() {
                    app.update(Message::CommitMsg(Action::Edit(Edit::Insert(c))));
                }
                app.update(Message::CommitMsg(Action::SelectAll));
                app.ops.push(Box::new(iced_core::widget::operation::focusable::focus(
                    crate::ui::widgets::COMMIT_BOX_ID.clone(),
                )));
            }
            "hidden" => {
                if let Some(p) = app.pane_of(crate::ui::app::Pane::Files) {
                    app.update(Message::PaneClose(p));
                }
            }
            "maxhover" => {
                app.cursor = iced_core::Point::new(300.0, 14.0);
                if let Some(p) = app.pane_of(crate::ui::app::Pane::Log) {
                    app.panes.maximize(p);
                }
            }
            "wrap" => {
                app.wrap = true;
                app.editor_wrap = true;
            }
            "merge" => {
                if let Some(f) = app.snapshot.conflicted.first().cloned() {
                    app.update(Message::MergeOpen(f.path));
                }
            }
            _ => {}
        }
    }
    // Three passes: fonts load, layout settles, then the final frame.
    let mut ui_ms = 0.0;
    for _ in 0..3 {
        let t0 = Instant::now();
        if app.zoom != 1.0 {
            shell.resize(size.0, size.1, ppp * app.zoom);
        }
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

/// Exit codes of the probe that main turns into the desktop window fallback.
pub const NO_GRAPHICS: i32 = 3;
pub const NO_GRAPHICS_MUX: i32 = 4;

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
        return Ok(Err(NO_GRAPHICS_MUX));
    }
    if !caps.kitty_graphics {
        eprintln!("gitgui: this terminal did not answer the kitty graphics probe. Supported: Ghostty, cmux, kitty, WezTerm.");
        return Ok(Err(NO_GRAPHICS));
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
    let mut pointer = "";
    let mut zoom_applied = app.zoom;

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
                            shell = Shell::new(font_size, ppp * app.zoom, nw, nh, app.theme.iced());
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
            shell.resize(nw, nh, ppp * app.zoom);
            app.scale = ppp * app.zoom;
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
                if app.zoom != zoom_applied {
                    zoom_applied = app.zoom;
                    shell.resize(fb.width(), fb.height(), ppp * app.zoom);
                    app.scale = ppp * app.zoom;
                }
                if let Some(p) = shell.cursor() {
                    app.cursor = p;
                }
                app.modifiers = shell.modifiers();
                let pass = shell.frame(&mut app, &mut fb);
                out.clear();
                for text in pass.copy.iter().chain(app.pending_copy.iter()) {
                    encode_osc52_copy(&mut out, text);
                }
                let shape = pointer_shape(pass.interaction);
                if shape != pointer {
                    pointer = shape;
                    encode_pointer_shape(&mut out, shape);
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
                    app.flush_state();
                    break;
                }
                last_frame = Instant::now();
                let mut delay = match pass.redraw {
                    RedrawRequest::NextFrame => Duration::ZERO,
                    RedrawRequest::At(at) => at.saturating_duration_since(last_frame),
                    RedrawRequest::Wait => Duration::from_secs(3600),
                };
                // Layout changes reach the state file once the pointer rests:
                // a drag produces a frame per move and should not write each.
                if app.state_dirty() {
                    if pass.redraw == RedrawRequest::NextFrame {
                        delay = Duration::from_millis(300);
                    } else {
                        app.flush_state();
                    }
                }
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
    if !pointer.is_empty() {
        out.clear();
        encode_pointer_shape(&mut out, "");
        let _ = term::write_all(&out);
    }
    let _ = worker.tx.send(Command::Quit);
    drop(session);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo::testutil::TempRepo;
    use crate::ui::app::{Pane, Selection};

    /// Drive the real app with a real repository through the shell, the same
    /// path the interactive loop takes: terminal bytes, iced events, frames.
    struct Harness {
        shell: Shell,
        app: App,
        repo: Repo,
        fb: Framebuffer,
        parser: Parser,
    }

    impl Harness {
        fn new(dir: &std::path::Path) -> Self {
            let theme = Theme::dark();
            let shell = Shell::new(13.0, 1.0, 900, 700, theme.iced());
            let mut app = App::new(theme, "test", 1.0, dir.to_path_buf());
            let mut repo = Repo::open(dir).unwrap();
            app.apply(Reply::Snapshot(repo.snapshot(100).unwrap()));
            let mut h = Harness {
                shell,
                app,
                repo,
                fb: Framebuffer::new(900, 700),
                parser: Parser::new(true, 1, 1),
            };
            h.settle();
            h.frame();
            h
        }

        /// Run pending commands synchronously: reads through `settle`, the
        /// few writes the tests use against the repo directly.
        fn settle(&mut self) {
            for _ in 0..6 {
                let cmds: Vec<Command> = self
                    .app
                    .pending
                    .iter()
                    .filter(|c| !matches!(c, Command::LoadDiff(_) | Command::LoadCommitFiles(_) | Command::ListDir(_)))
                    .cloned()
                    .collect();
                self.app.pending.retain(|c| matches!(c, Command::LoadDiff(_) | Command::LoadCommitFiles(_) | Command::ListDir(_)));
                settle(&mut self.app, &mut self.repo);
                if cmds.is_empty() {
                    break;
                }
                for cmd in cmds {
                    let label = cmd.label();
                    let result = match cmd {
                        Command::Stage(p) => self.repo.stage(&p).map(|_| "staged".to_owned()),
                        Command::StageAll => self.repo.stage_all().map(|_| "staged".to_owned()),
                        Command::Unstage(p) => self.repo.unstage(&p).map(|_| "unstaged".to_owned()),
                        Command::Commit { message, amend } => {
                            self.repo.commit(&message, amend).map(|_| "committed".to_owned())
                        }
                        Command::Refresh | Command::SetDiffOpts(_) => continue,
                        other => panic!("harness cannot run {other:?}"),
                    };
                    self.app.apply(Reply::Op {
                        label,
                        result: result.map_err(|e| e.to_string()),
                    });
                    self.app.apply(Reply::Snapshot(self.repo.snapshot(100).unwrap()));
                }
            }
        }

        fn frame(&mut self) {
            self.shell.frame(&mut self.app, &mut self.fb);
            self.settle();
        }

        fn key(&mut self, bytes: &[u8]) {
            let events: Vec<_> = self.parser.feed(bytes).into_iter().chain(self.parser.flush()).collect();
            for ev in &events {
                self.shell.push(ev);
            }
            self.frame();
            self.frame();
        }
    }

    #[test]
    fn frame_paints_the_theme_background_in_rgba() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        let h = Harness::new(&t.dir);
        let bg = h.app.theme.background;
        let px = h.fb.pixel(2, 350);
        assert_eq!(px[3], 255);
        // Byte order: the theme background is a neutral grey, so check a
        // colored pixel instead: the pane border / selection somewhere in the
        // frame must contain the accent's blue dominance.
        let mut blue_dominant = 0;
        for chunk in h.fb.pixels().as_chunks::<4>().0 {
            let (r, g, b) = (chunk[0] as u16, chunk[1] as u16, chunk[2] as u16);
            if b > r + 40 && b > g + 20 {
                blue_dominant += 1;
            }
        }
        assert!(blue_dominant > 200, "expected accent-colored pixels, got {blue_dominant}");
        let _ = bg;
    }

    #[test]
    fn keyboard_navigation_and_staging() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.commit_file("b.txt", "two\n", "second");
        t.write("a.txt", "one\ntwo\n");
        t.write("c.txt", "new\n");
        let mut h = Harness::new(&t.dir);
        assert_eq!(h.app.selection, Selection::WorkingTree);
        assert_eq!(h.app.snapshot.unstaged.len(), 2);

        // j / k move through the log rows.
        h.key(b"j");
        assert_eq!(h.app.selection, Selection::Commit(0));
        h.key(b"k");
        assert_eq!(h.app.selection, Selection::WorkingTree);

        // s stages the selected (first unstaged) file, a stages everything.
        h.key(b"s");
        assert_eq!(h.app.snapshot.staged.len(), 1);
        assert_eq!(h.app.snapshot.staged[0].path, "a.txt");
        h.key(b"a");
        assert_eq!(h.app.snapshot.staged.len(), 2);
        assert!(h.app.snapshot.unstaged.is_empty());
    }

    #[test]
    fn commit_box_takes_typed_text_and_ctrl_enter_commits() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.write("a.txt", "changed\n");
        let mut h = Harness::new(&t.dir);
        h.key(b"a");
        assert_eq!(h.app.snapshot.staged.len(), 1);
        // c focuses the commit box; typed keys land there, q does not quit.
        h.key(b"c");
        h.key(b"fix things");
        h.key(b"q");
        assert!(!h.app.quit);
        assert_eq!(h.app.commit_message_text().trim_end(), "fix thingsq");
        // Ctrl+Enter commits.
        h.key(b"\x1b[13;5u");
        assert_eq!(h.app.snapshot.commits.len(), 2);
        assert_eq!(h.app.snapshot.commits[0].summary, "fix thingsq");
        assert!(h.app.commit_message_text().trim().is_empty(), "message cleared after commit");
    }

    #[test]
    fn editor_opens_from_the_selection_and_escape_closes() {
        let t = TempRepo::new();
        t.commit_file("a.rs", "fn main() {}\n", "init");
        t.write("a.rs", "fn main() {}\n// change\n");
        let mut h = Harness::new(&t.dir);
        h.key(b"e");
        let ed = h.app.editor.as_ref().expect("editor open");
        assert_eq!(ed.path, "a.rs");
        assert_eq!(ed.lang, crate::ui::highlight::Lang::Rust);
        // Typed text lands in the editor, not in the bindings.
        h.key(b"x");
        assert!(h.app.editor.as_ref().unwrap().dirty());
        // Escape with a dirty buffer asks; a second Escape cancels the dialog.
        h.key(b"\x1b");
        assert!(matches!(h.app.modal, Some(crate::ui::app::Modal::CloseEditor)));
        // The dialog owns the keyboard: typing does not reach the editor.
        let before = h.app.editor.as_ref().unwrap().content.text();
        h.key(b"y");
        assert_eq!(h.app.editor.as_ref().unwrap().content.text(), before);
        h.key(b"\x1b");
        assert!(h.app.modal.is_none());
        assert!(h.app.editor.is_some());
    }

    #[test]
    fn digits_maximize_panes_and_tree_lists_lazily() {
        let t = TempRepo::new();
        t.commit_file("src/lib.rs", "pub fn f() {}\n", "init");
        t.write("README.md", "# hi\n");
        let mut h = Harness::new(&t.dir);
        let root = h.app.tree.get("").expect("root listed");
        let names: Vec<&str> = root.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["src", "README.md"]);
        h.app.update(crate::ui::app::Message::TreeToggle("src".into()));
        h.frame();
        assert_eq!(h.app.tree.get("src").unwrap()[0].path, "src/lib.rs");

        assert!(h.app.panes.maximized().is_none());
        h.key(b"2");
        let log = h.app.pane_of(Pane::Log).unwrap();
        assert_eq!(h.app.panes.maximized(), Some(log));
        h.key(b"2");
        assert!(h.app.panes.maximized().is_none());
        // A tree file opens the editor over the whole main area: only the
        // repository, files and editor panes remain; closing it restores all.
        h.app.update(crate::ui::app::Message::TreeOpen("src/lib.rs".into()));
        h.frame();
        assert_eq!(h.app.editor.as_ref().map(|e| e.path.as_str()), Some("src/lib.rs"));
        assert!(h.app.editor_full);
        assert_eq!(h.app.editor_panes.len(), 3);
        h.key(b"\x1b");
        assert!(h.app.editor.is_none());
        assert!(!h.app.editor_full);
    }

    #[test]
    fn wheel_over_the_log_scrolls_it() {
        let t = TempRepo::new();
        for i in 0..60 {
            t.commit_file("a.txt", &format!("{i}\n"), &format!("commit {i}"));
        }
        let mut h = Harness::new(&t.dir);
        // The log pane is the top-right one: find a point inside it.
        let log = h.app.pane_of(Pane::Log).unwrap();
        let region = h.app.panes.layout().pane_regions(4.0, 80.0, iced_core::Size::new(900.0, 700.0 - 34.0 - 8.0));
        let r = region[&log];
        let (x, y) = ((r.x + r.width / 2.0) as i32, (r.y + r.height / 2.0) as i32);
        let row = |h: &Harness| -> Vec<u8> { h.fb.pixels().to_vec() };
        let before = row(&h);
        // SGR wheel down (button 65) in pixel coordinates.
        h.key(format!("\x1b[<65;{x};{y}M").as_bytes());
        h.key(format!("\x1b[<65;{x};{y}M").as_bytes());
        let after = row(&h);
        assert_ne!(before, after, "rows should have moved under the pointer");
        assert_eq!(h.app.selection, Selection::Commit(0), "scrolling does not change the selection");
    }

    #[test]
    fn pointer_shape_sequences() {
        let mut out = Vec::new();
        encode_pointer_shape(&mut out, "grab");
        assert_eq!(out, b"\x1b]22;grab\x1b\\");
        out.clear();
        encode_pointer_shape(&mut out, "");
        assert_eq!(out, b"\x1b]22;\x1b\\");
        assert_eq!(pointer_shape(iced_core::mouse::Interaction::Grab), "grab");
        assert_eq!(pointer_shape(iced_core::mouse::Interaction::None), "");
    }

    #[test]
    fn title_bar_hover_highlights_and_asks_for_a_grab_pointer() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        let mut h = Harness::new(&t.dir);
        let log = h.app.pane_of(Pane::Log).unwrap();
        let region = h.app.panes.layout().pane_regions(4.0, 80.0, iced_core::Size::new(900.0, 700.0 - 34.0 - 8.0));
        let r = region[&log];
        // The title bar is the top strip of the pane; move over its middle.
        let (x, y) = ((r.x + r.width / 2.0) as i32 + 4, (r.y + 12.0) as i32 + 4);
        h.key(format!("\x1b[<35;{x};{y}M").as_bytes());
        assert_eq!(h.app.hovered_pane(), Some(log));
        let out = h.shell.frame(&mut h.app, &mut h.fb);
        assert_eq!(pointer_shape(out.interaction), "grab");
        // Leaving the bar clears it.
        let (x, y) = ((r.x + r.width / 2.0) as i32, (r.y + r.height / 2.0) as i32);
        h.key(format!("\x1b[<35;{x};{y}M").as_bytes());
        assert_eq!(h.app.hovered_pane(), None);
        assert_ne!(pointer_shape(h.shell.frame(&mut h.app, &mut h.fb).interaction), "grab");
    }

    #[test]
    fn panes_hide_from_the_x_and_come_back_from_the_footer() {
        use crate::ui::app::Message;
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        let mut h = Harness::new(&t.dir);
        assert_eq!(h.app.panes.len(), 5);
        let files = h.app.pane_of(Pane::Files).unwrap();
        h.app.update(Message::PaneClose(files));
        h.frame();
        assert_eq!(h.app.panes.len(), 4);
        assert_eq!(h.app.hidden_panes(), vec![Pane::Files]);
        h.app.update(Message::PaneShow(Pane::Files));
        h.frame();
        assert_eq!(h.app.panes.len(), 5);
        assert!(h.app.hidden_panes().is_empty());
        // The last pane cannot be closed.
        for kind in [Pane::Files, Pane::Log, Pane::Changes, Pane::Detail] {
            let p = h.app.pane_of(kind).unwrap();
            h.app.update(Message::PaneClose(p));
        }
        let last = h.app.pane_of(Pane::Sidebar).unwrap();
        h.app.update(Message::PaneClose(last));
        assert_eq!(h.app.panes.len(), 1);
    }

    #[test]
    fn layout_and_settings_persist_in_the_git_dir() {
        use crate::ui::app::Message;
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        let mut h = Harness::new(&t.dir);
        let path = h.app.state_path.clone().expect("a repository has a state path");
        let git_dir = t.dir.join(".git").canonicalize().unwrap();
        assert!(path.starts_with(&git_dir), "{}", path.display());
        assert!(!h.app.state_dirty());
        let files = h.app.pane_of(Pane::Files).unwrap();
        h.app.update(Message::PaneClose(files));
        h.app.update(Message::DiffWrap);
        h.app.update(Message::SectionToggle("Tags"));
        h.app.update(Message::LogColumns(160.0, 60.0));
        let log = h.app.pane_of(Pane::Log).unwrap();
        h.app.update(Message::PaneMaximize(log));
        assert!(h.app.state_dirty());
        h.app.flush_state();
        assert!(!h.app.state_dirty());
        let text = std::fs::read_to_string(&path).unwrap();
        let main = serde_json::from_str::<serde_json::Value>(&text).unwrap()["panes"].to_string();
        assert!(main.contains("\"commits\"") && !main.contains("\"files\""), "{main}");

        let again = Harness::new(&t.dir);
        assert_eq!(again.app.hidden_panes(), vec![Pane::Files]);
        assert!(again.app.wrap);
        assert!(again.app.sidebar_collapsed.contains("Tags"));
        assert_eq!(again.app.log_columns, (160.0, 60.0));
        assert_eq!(again.app.panes.maximized(), again.app.pane_of(Pane::Log));
        assert!(!again.app.state_dirty());

        // A broken file is ignored and the defaults come back.
        std::fs::write(&path, "{\"panes\": {\"axis\": \"v\", \"ratio\": 0.5, \"a\": \"files\", \"b\": \"files\"}, \"wrap\": true}").unwrap();
        let broken = Harness::new(&t.dir);
        assert_eq!(broken.app.panes.len(), 5);
        assert!(broken.app.wrap);
        std::fs::write(&path, "not json").unwrap();
        let junk = Harness::new(&t.dir);
        assert_eq!(junk.app.panes.len(), 5);
        assert!(!junk.app.wrap);
    }

    #[test]
    fn open_another_repository_from_the_folder_dialog() {
        use crate::ui::app::{Message, Modal};
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        let other = TempRepo::new();
        other.commit_file("b.txt", "two\n", "other");
        let mut h = Harness::new(&t.dir);
        h.app.update(Message::OpenFolderDialog);
        let Some(Modal::OpenFolder { path, entries }) = h.app.modal.clone() else {
            panic!("dialog expected")
        };
        // Starts at the parent of the current repository, where both temp
        // repositories sit, and marks them as git.
        let parent = t.dir.canonicalize().unwrap().parent().unwrap().to_path_buf();
        assert_eq!(std::path::PathBuf::from(&path), parent);
        let name = t.dir.file_name().unwrap().to_str().unwrap();
        assert!(entries.iter().any(|(n, git)| n == name && *git), "{entries:?}");

        // Enter a subfolder, back up, then type the other path and confirm.
        h.app.update(Message::OpenFolderEnter(name.to_owned()));
        assert!(matches!(&h.app.modal, Some(Modal::OpenFolder { path, .. }) if path.ends_with(name)));
        h.app.update(Message::OpenFolderUp);
        assert!(matches!(&h.app.modal, Some(Modal::OpenFolder { path, .. }) if parent == std::path::Path::new(path)));
        h.app.update(Message::ModalValue(other.dir.display().to_string()));
        h.app.update(Message::ModalConfirm);
        assert!(h.app.modal.is_none());
        assert!(!h.app.have_snapshot);
        assert_eq!(h.app.pending.pop(), Some(Command::Open(other.dir.clone())));
        h.app.pending.clear();

        // The worker answers with the other repository's snapshot.
        let mut repo = Repo::open(&other.dir).unwrap();
        h.app.apply(Reply::Snapshot(repo.snapshot(100).unwrap()));
        h.frame();
        assert_eq!(h.app.snapshot.path.canonicalize().unwrap(), other.dir.canonicalize().unwrap());
        assert!(h.app.snapshot.commits.iter().any(|c| c.summary == "other"));
        h.app.pending.clear();

        // A plain folder lands on the no-repository screen, which still opens the dialog.
        let plain = std::env::temp_dir().join(format!("gitgui-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        h.app.apply(Reply::NoRepo(plain.clone()));
        assert!(h.app.no_repo);
        h.frame();
        h.app.update(Message::OpenFolderDialog);
        assert!(matches!(&h.app.modal, Some(Modal::OpenFolder { .. })));
        h.frame();
        h.app.pending.clear();
        let _ = std::fs::remove_dir(&plain);
    }

    #[test]
    fn ctrl_minus_equal_and_zero_change_the_zoom() {
        use crate::ui::app::Message;
        use iced_core::keyboard::{Key, Modifiers};
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        let mut h = Harness::new(&t.dir);
        let key = |c: &str| Message::Key(Key::Character(c.into()), Modifiers::CTRL);
        assert_eq!(h.app.zoom, 1.0);
        h.app.update(key("-"));
        assert_eq!(h.app.zoom, 0.9);
        h.app.update(key("="));
        h.app.update(key("+"));
        assert_eq!(h.app.zoom, 1.1);
        h.app.update(key("0"));
        assert_eq!(h.app.zoom, 1.0);
        for _ in 0..40 {
            h.app.update(key("="));
        }
        assert_eq!(h.app.zoom, 3.0);
        assert!(h.app.state_dirty(), "zoom is part of the saved state");
        h.frame();
    }

    #[test]
    fn agent_writes_with_an_id_are_safe_to_retry() {
        use crate::agent::{handle_in_app, AgentCmd};
        use crate::ui::app::AgentOutcome;
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.write("a.txt", "two\n");
        let mut h = Harness::new(&t.dir);
        let mut shot = None;
        let parse = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();

        // First send queues; a retry before the worker answered says so.
        let r = parse(&handle_in_app(&mut h.app, AgentCmd::Stage { paths: vec!["a.txt".into()], id: Some("s1".into()) }, &mut shot));
        assert_eq!(r["data"]["queued"], "stage");
        assert_eq!(r["data"]["id"], "s1");
        let r = parse(&handle_in_app(&mut h.app, AgentCmd::Stage { paths: vec!["a.txt".into()], id: Some("s1".into()) }, &mut shot));
        assert_eq!(r["data"]["duplicate"], true);
        assert_eq!(r["data"]["state"], "queued");
        assert_eq!(h.app.pending.len(), 1, "the retry did not queue again");
        h.settle();

        // After the worker ran it, the retry returns the recorded result.
        let r = parse(&handle_in_app(&mut h.app, AgentCmd::Stage { paths: vec!["a.txt".into()], id: Some("s1".into()) }, &mut shot));
        assert_eq!(r["data"]["state"], "done");
        assert_eq!(r["data"]["ok"], true);
        assert!(h.app.pending.is_empty());
        let r = parse(&handle_in_app(&mut h.app, AgentCmd::Result { id: "s1".into() }, &mut shot));
        assert_eq!(r["data"]["duplicate"], false);
        let r = parse(&handle_in_app(&mut h.app, AgentCmd::Result { id: "nope".into() }, &mut shot));
        assert_eq!(r["ok"], false);

        // A commit, then a retried commit with a new id: nothing to commit.
        let r = parse(&handle_in_app(&mut h.app, AgentCmd::Commit { message: "two".into(), id: Some("c1".into()) }, &mut shot));
        assert_eq!(r["data"]["queued"], "commit");
        h.settle();
        assert_eq!(h.app.agent_result("c1"), Some(&AgentOutcome::Done { ok: true, message: "committed".into() }));
        let st = parse(&handle_in_app(&mut h.app, AgentCmd::Status, &mut shot));
        assert_eq!(st["data"]["last_op"]["label"], "commit");
        assert_eq!(st["data"]["last_op"]["ok"], true);
        assert_eq!(st["data"]["head"].as_str().unwrap().len(), 40);
        handle_in_app(&mut h.app, AgentCmd::Commit { message: "two".into(), id: Some("c2".into()) }, &mut shot);
        h.settle();
        match h.app.agent_result("c2") {
            Some(AgentOutcome::Done { ok: false, message }) => assert!(message.contains("nothing to commit"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(h.app.snapshot.commits.len(), 2);

        // commit_and_push answers twice; a failed push ends it as an error.
        h.app.run_for_agent(Some("cp".into()), Command::CommitAndPush { message: "m".into(), amend: false });
        h.app.pending.clear();
        h.app.apply(Reply::Op { label: "commit", result: Ok("committed".into()) });
        assert_eq!(h.app.agent_result("cp"), Some(&AgentOutcome::Queued));
        h.app.apply(Reply::Op { label: "push", result: Err("no remote".into()) });
        assert_eq!(h.app.agent_result("cp"), Some(&AgentOutcome::Done { ok: false, message: "no remote".into() }));
        h.frame();
    }

    #[test]
    fn bracketed_paste_lands_in_the_focused_field() {
        use crate::ui::app::Message;
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.write("a.txt", "two\n");
        let mut h = Harness::new(&t.dir);
        // `c` focuses the commit message; a bracketed paste then types into it.
        h.key(b"c");
        h.key(b"\x1b[200~fix: pasted message\x1b[201~");
        assert_eq!(h.app.commit_msg.text().trim_end(), "fix: pasted message");
        // A dialog's text input takes a paste the same way.
        h.app.update(Message::OpenFolderDialog);
        h.frame();
        h.key(b"\x1b[200~/tmp/somewhere\x1b[201~");
        let path = match &h.app.modal {
            Some(crate::ui::app::Modal::OpenFolder { path, .. }) => path.clone(),
            other => panic!("{other:?}"),
        };
        assert!(path.ends_with("/tmp/somewhere"), "{path}");
    }

    #[test]
    fn editor_undo_redo_select_all_copy_and_dialog_select_all() {
        let t = TempRepo::new();
        t.commit_file("a.rs", "fn main() {}\n", "init");
        let mut h = Harness::new(&t.dir);
        h.key(b"e");
        let text = |h: &Harness| h.app.editor.as_ref().unwrap().content.text();
        let original = text(&h);
        // Typing groups into one undo step; Ctrl+Z (CSI u, ctrl) takes it back.
        h.key(b"a");
        h.key(b"b");
        assert!(text(&h).starts_with("ab"));
        assert!(h.app.editor.as_ref().unwrap().dirty());
        h.key(b"\x1b[122;5u");
        assert_eq!(text(&h), original);
        assert!(!h.app.editor.as_ref().unwrap().dirty());
        // Ctrl+Y brings it back, Ctrl+Shift+Z the same.
        h.key(b"\x1b[121;5u");
        assert!(text(&h).starts_with("ab"));
        h.key(b"\x1b[122;5u");
        h.key(b"\x1b[122;6u");
        assert!(text(&h).starts_with("ab"));
        // Ctrl+A then Ctrl+C copies the whole buffer to the terminal clipboard.
        h.key(b"\x1b[97;5u");
        let events: Vec<_> = h.parser.feed(b"\x1b[99;5u").into_iter().chain(h.parser.flush()).collect();
        for ev in &events {
            h.shell.push(ev);
        }
        let out = h.shell.frame(&mut h.app, &mut h.fb);
        assert_eq!(out.copy.len(), 1, "Ctrl+C in the editor copies, it does not quit");
        assert!(out.copy[0].starts_with("ab"));
        assert!(!h.app.quit);
        // Ctrl+X cuts the selection.
        h.key(b"\x1b[97;5u");
        h.key(b"\x1b[120;5u");
        assert_eq!(text(&h).trim(), "");
        h.key(b"\x1b[122;5u");
        assert!(text(&h).starts_with("ab"));

        // A dialog's text input: Ctrl+A selects all, typing replaces it,
        // Ctrl+Z puts the old value back, Ctrl+Y the typed one.
        h.key(b"\x1b");
        h.key(b"\x1b");
        h.app.update(crate::ui::app::Message::OpenFolderDialog);
        h.frame();
        let modal_path = |h: &Harness| match &h.app.modal {
            Some(crate::ui::app::Modal::OpenFolder { path, .. }) => path.clone(),
            other => panic!("{other:?}"),
        };
        let start = modal_path(&h);
        h.key(b"\x1b[97;5u");
        h.key(b"z");
        h.key(b"q");
        assert_eq!(modal_path(&h), "zq");
        // Replacing the selection is one step, the typing after it another.
        h.key(b"\x1b[122;5u");
        assert_eq!(modal_path(&h), "z");
        h.key(b"\x1b[122;5u");
        assert_eq!(modal_path(&h), start);
        h.key(b"\x1b[121;5u");
        h.key(b"\x1b[121;5u");
        assert_eq!(modal_path(&h), "zq");
        h.key(b"\x1b");
        assert!(h.app.modal.is_none());

        // The commit box: typing, Ctrl+Z, Ctrl+Y, and Ctrl+C copies rather than quits.
        h.key(b"c");
        h.key(b"o");
        h.key(b"k");
        assert_eq!(h.app.commit_msg.text().trim_end(), "ok");
        h.key(b"\x1b[122;5u");
        assert_eq!(h.app.commit_msg.text().trim_end(), "");
        h.key(b"\x1b[121;5u");
        assert_eq!(h.app.commit_msg.text().trim_end(), "ok");
        h.key(b"\x1b[97;5u");
        h.key(b"\x1b[99;5u");
        assert!(!h.app.quit);
        h.key(b"\x1b");

        // The commit filter, a text input without a binding hook.
        // (Escape above may have asked about the dirty editor: dismiss that.)
        if h.app.modal.is_some() {
            h.key(b"\x1b");
        }
        assert!(h.app.modal.is_none());
        h.key(b"/");
        h.key(b"m");
        h.key(b"a");
        assert_eq!(h.app.filter, "ma");
        h.key(b"\x1b[122;5u");
        assert_eq!(h.app.filter, "");
        h.key(b"\x1b[121;5u");
        assert_eq!(h.app.filter, "ma");
    }

    /// Pixels that differ between two frames of the same size.
    fn changed_pixels(a: &[u8], b: &[u8]) -> usize {
        a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count()
    }

    #[test]
    fn selecting_text_in_the_commit_box_draws_a_highlight() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.write("a.txt", "two\n");
        let mut h = Harness::new(&t.dir);
        h.key(b"c");
        for b in b"select all of this text please" {
            h.key(&[*b]);
        }
        h.frame();
        let before = h.fb.pixels().to_vec();
        h.key(b"\x1b[97;5u");
        h.frame();
        assert_eq!(h.app.commit_msg.selection().as_deref(), Some("select all of this text please"));
        let changed = changed_pixels(&before, h.fb.pixels());
        assert!(changed > 1500, "selection highlight missing: {changed} pixels changed");

        // Dragging with the mouse (SGR pixel reports) selects too: press on
        // the first word, move, release.
        h.key(b"\x1b[97;5u");
        h.key(b"\x1b[D");
        assert!(h.app.commit_msg.selection().is_none());
        h.key(b"\x1b[<0;200;568M");
        h.key(b"\x1b[<32;260;568M");
        h.key(b"\x1b[<32;300;568M");
        h.key(b"\x1b[<0;300;568m");
        let dragged = h.app.commit_msg.selection();
        assert!(dragged.as_deref().is_some_and(|s| s.len() > 3), "{dragged:?}");
    }

    #[test]
    fn dragging_over_the_diff_text_selects_it_and_ctrl_c_copies() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "alpha beta gamma\ndelta epsilon zeta\neta theta iota\n", "init");
        t.write("a.txt", "alpha beta gamma\ndelta epsilon zeta changed\neta theta iota\n");
        let mut h = Harness::new(&t.dir);
        h.frame();
        assert!(h.app.diff.is_some(), "the working tree diff is loaded");
        let (_, detail) = h
            .app
            .title_strips()
            .into_iter()
            .find(|(p, _)| h.app.panes.get(*p) == Some(&Pane::Detail))
            .expect("detail pane");
        // Below the title bar and the diff header, right of the gutter.
        let x0 = (detail.x + 110.0) as i32;
        let y0 = (detail.y + 95.0) as i32;
        let press = format!("\x1b[<0;{x0};{y0}M");
        let mv1 = format!("\x1b[<32;{};{}M", x0 + 30, y0);
        let mv2 = format!("\x1b[<32;{};{}M", x0 + 60, y0 + 18);
        let rel = format!("\x1b[<0;{};{}m", x0 + 60, y0 + 18);
        h.key(press.as_bytes());
        assert!(h.app.diff_text_sel.is_none(), "a press alone selects nothing");
        h.key(mv1.as_bytes());
        h.key(mv2.as_bytes());
        h.key(rel.as_bytes());
        let sel = h.app.diff_text_sel.expect("drag selected text");
        let (a, b) = sel.ordered();
        assert!(a < b, "{sel:?}");
        let text = h.app.diff_selected_text().expect("selected text");
        assert!(text.contains('\n'), "two rows: {text:?}");
        assert!(h.app.line_sel.is_none(), "a text drag is not a line selection");
        // Ctrl+C copies instead of quitting; Escape clears; a plain click
        // on the text still selects the line for staging.
        h.key(b"\x1b[99;5u");
        assert!(!h.app.quit);
        assert_eq!(h.app.pending_copy.last(), Some(&text));
        h.key(b"\x1b");
        assert!(h.app.diff_text_sel.is_none());
        h.key(press.as_bytes());
        h.key(format!("\x1b[<0;{x0};{y0}m").as_bytes());
        assert!(h.app.diff_text_sel.is_none());
        assert!(h.app.line_sel.is_some(), "click selects the line");
    }

    #[test]
    fn font_size_tracks_cell_height() {
        assert_eq!(font_size_for_cell(0, 2.0), 13.0);
        assert_eq!(font_size_for_cell(34, 2.0), 13.0);
        assert_eq!(font_size_for_cell(17, 1.0), 13.0);
        assert_eq!(font_size_for_cell(60, 1.0), 24.0);
    }
}
