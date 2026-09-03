//! Unix socket JSON-lines control API for coding agents (docs/SPEC.md section 7).

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::git::ops::Command;
use crate::ui::app::{App, Selection};

/// Where instance sockets and metadata live.
pub fn socket_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("gitgui");
    }
    std::env::temp_dir().join("gitgui")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub pid: u32,
    pub repo: PathBuf,
    pub tty: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum AgentCmd {
    Status,
    Select { oid: String },
    Stage { paths: Vec<String> },
    Unstage { paths: Vec<String> },
    Commit { message: String },
    #[serde(rename = "commit_and_push")]
    CommitAndPush {
        message: String,
        #[serde(default)]
        amend: bool,
    },
    Fetch,
    Pull,
    Push,
    Screenshot { path: String },
    List,
}

#[derive(Debug)]
pub struct AgentJob {
    pub request: AgentCmd,
    pub reply: mpsc::Sender<String>,
}

pub struct Server {
    sock_path: PathBuf,
    meta_path: PathBuf,
}

impl Server {
    pub fn bind(repo: &Path, tx: mpsc::Sender<AgentJob>) -> Result<Self> {
        let dir = socket_dir();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let pid = std::process::id();
        let sock_path = dir.join(format!("{pid}.sock"));
        let meta_path = dir.join(format!("{pid}.json"));
        let _ = fs::remove_file(&sock_path);
        let meta = InstanceMeta {
            pid,
            repo: repo.to_path_buf(),
            tty: controlling_tty(),
        };
        fs::write(&meta_path, serde_json::to_vec_pretty(&meta)?)?;
        let listener = UnixListener::bind(&sock_path)
            .with_context(|| format!("bind {}", sock_path.display()))?;
        listener
            .set_nonblocking(true)
            .context("set_nonblocking on agent socket")?;
        let sock_path_c = sock_path.clone();
        std::thread::Builder::new()
            .name("agent".into())
            .spawn(move || accept_loop(listener, tx, sock_path_c))
            .context("spawn agent thread")?;
        Ok(Self {
            sock_path,
            meta_path,
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.sock_path);
        let _ = fs::remove_file(&self.meta_path);
    }
}

fn accept_loop(listener: UnixListener, tx: mpsc::Sender<AgentJob>, sock_path: PathBuf) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(e) = handle_client(stream, &tx) {
                    eprintln!("gitgui agent: {e:#}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
        if !sock_path.exists() {
            break;
        }
    }
}

fn handle_client(stream: UnixStream, tx: &mpsc::Sender<AgentJob>) -> Result<()> {
    let reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cmd: AgentCmd = serde_json::from_str(line).context("parse agent command")?;
        if matches!(cmd, AgentCmd::List) {
            let resp = ok(list_instances()?);
            writeln!(stream, "{resp}")?;
            stream.flush()?;
            continue;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(AgentJob {
            request: cmd,
            reply: reply_tx,
        })
        .map_err(|_| anyhow::anyhow!("gitgui is shutting down"))?;
        match reply_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(resp) => {
                writeln!(stream, "{resp}")?;
                stream.flush()?;
            }
            Err(_) => {
                writeln!(stream, "{}", err("agent request timed out"))?;
                stream.flush()?;
            }
        }
    }
    Ok(())
}

pub fn ok(data: Value) -> String {
    serde_json::to_string(&json!({ "ok": true, "data": data })).unwrap()
}

pub fn err(msg: impl Into<String>) -> String {
    serde_json::to_string(&json!({ "ok": false, "error": msg.into() })).unwrap()
}

pub fn handle_in_app(app: &mut App, cmd: AgentCmd, screenshot: &mut Option<PathBuf>) -> String {
    match cmd {
        AgentCmd::Status => ok(status_json(app)),
        AgentCmd::Select { oid } => match select_oid(app, &oid) {
            Ok(()) => ok(json!({ "selected": selection_label(app) })),
            Err(e) => err(e),
        },
        AgentCmd::Stage { paths } => {
            if paths.is_empty() {
                return err("paths is empty");
            }
            app.run(Command::Stage(paths));
            ok(json!({ "queued": "stage" }))
        }
        AgentCmd::Unstage { paths } => {
            if paths.is_empty() {
                return err("paths is empty");
            }
            app.run(Command::Unstage(paths));
            ok(json!({ "queued": "unstage" }))
        }
        AgentCmd::Commit { message } => {
            if message.trim().is_empty() {
                return err("message is empty");
            }
            app.run(Command::Commit {
                message,
                amend: false,
            });
            ok(json!({ "queued": "commit" }))
        }
        AgentCmd::CommitAndPush { message, amend } => {
            if message.trim().is_empty() {
                return err("message is empty");
            }
            app.run(Command::CommitAndPush { message, amend });
            ok(json!({ "queued": "commit_and_push" }))
        }
        AgentCmd::Fetch => {
            app.run(Command::Fetch);
            ok(json!({ "queued": "fetch" }))
        }
        AgentCmd::Pull => {
            app.run(Command::Pull);
            ok(json!({ "queued": "pull" }))
        }
        AgentCmd::Push => {
            app.run(Command::Push);
            ok(json!({ "queued": "push" }))
        }
        AgentCmd::Screenshot { path } => {
            screenshot.replace(PathBuf::from(path));
            ok(json!({ "queued": "screenshot" }))
        }
        AgentCmd::List => ok(list_instances().unwrap_or_else(|e| json!({ "error": e.to_string() }))),
    }
}

fn status_json(app: &App) -> Value {
    json!({
        "pid": std::process::id(),
        "repo": app.snapshot.path,
        "branch": app.snapshot.head.as_ref().and_then(|h| h.branch_name.clone()),
        "detached": app.snapshot.head.as_ref().is_some_and(|h| h.detached),
        "staged": app.snapshot.staged.len(),
        "unstaged": app.snapshot.unstaged.len(),
        "conflicted": app.snapshot.conflicted.len(),
        "dirty": app.snapshot.is_dirty(),
        "selected": selection_label(app),
        "busy": app.busy,
    })
}

fn selection_label(app: &App) -> Value {
    match app.selection {
        Selection::WorkingTree => json!("working-tree"),
        Selection::Commit(i) => app
            .snapshot
            .commits
            .get(i)
            .map(|c| json!({ "oid": c.short, "summary": c.summary }))
            .unwrap_or(json!(null)),
    }
}

fn select_oid(app: &mut App, prefix: &str) -> Result<(), String> {
    let prefix = prefix.trim().to_lowercase();
    if prefix.is_empty() {
        return Err("oid is empty".into());
    }
    if prefix == "working-tree" || prefix == "worktree" {
        app.select(Selection::WorkingTree);
        return Ok(());
    }
    let idx = app
        .snapshot
        .commits
        .iter()
        .position(|c| {
            c.short.to_lowercase().starts_with(&prefix)
                || c.oid.to_string().starts_with(&prefix)
        })
        .ok_or_else(|| format!("no commit matching {prefix}"))?;
    app.select(Selection::Commit(idx));
    Ok(())
}

pub fn list_instances() -> Result<Value> {
    let dir = socket_dir();
    let mut rows = Vec::new();
    if !dir.is_dir() {
        return Ok(json!(rows));
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(OsStr::to_str) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let meta: InstanceMeta = serde_json::from_str(&text)?;
        let sock = dir.join(format!("{}.sock", meta.pid));
        rows.push(json!({
            "pid": meta.pid,
            "repo": meta.repo,
            "tty": meta.tty,
            "socket": sock,
            "alive": sock.exists(),
        }));
    }
    rows.sort_by_key(|v| v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0));
    Ok(json!(rows))
}

pub fn run_ls() -> Result<i32> {
    let rows = list_instances()?;
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(0)
}

pub fn run_action(json_line: &str, pid: Option<u32>) -> Result<i32> {
    let cmd: AgentCmd = serde_json::from_str(json_line).context("parse action json")?;
    if matches!(cmd, AgentCmd::List) {
        return run_ls();
    }
    let sock = resolve_socket(pid)?;
    let mut stream = UnixStream::connect(&sock)
        .with_context(|| format!("connect to {}", sock.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "{json_line}")?;
    let reader = BufReader::new(stream);
    if let Some(line) = reader.lines().next() {
        println!("{}", line?);
    }
    Ok(0)
}

fn resolve_socket(pid: Option<u32>) -> Result<PathBuf> {
    if let Some(pid) = pid {
        let sock = socket_dir().join(format!("{pid}.sock"));
        if sock.exists() {
            return Ok(sock);
        }
        bail!("no gitgui instance with pid {pid}");
    }
    if let Some(tty) = controlling_tty() {
        let dir = socket_dir();
        if dir.is_dir() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(OsStr::to_str) != Some("json") {
                    continue;
                }
                let meta: InstanceMeta = serde_json::from_str(&fs::read_to_string(&path)?)?;
                if meta.tty.as_deref() == Some(tty.as_str()) {
                    let sock = dir.join(format!("{}.sock", meta.pid));
                    if sock.exists() {
                        return Ok(sock);
                    }
                }
            }
        }
    }
    bail!("no gitgui instance for this terminal; run `gitgui ls`")
}

fn controlling_tty() -> Option<String> {
    unsafe {
        let name = libc::ttyname(libc::STDIN_FILENO);
        if name.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(name).to_string_lossy().into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::App;
    use crate::ui::theme::Theme;

    #[test]
    fn parse_agent_commands() {
        let s: AgentCmd = serde_json::from_str(r#"{"cmd":"status"}"#).unwrap();
        assert!(matches!(s, AgentCmd::Status));
        let s: AgentCmd =
            serde_json::from_str(r#"{"cmd":"stage","paths":["a.rs","b.rs"]}"#).unwrap();
        assert!(matches!(s, AgentCmd::Stage { .. }));
        let s: AgentCmd = serde_json::from_str(r#"{"cmd":"commit_and_push","message":"fix"}"#).unwrap();
        assert!(matches!(s, AgentCmd::CommitAndPush { .. }));
        let s: AgentCmd = serde_json::from_str(r#"{"cmd":"push"}"#).unwrap();
        assert!(matches!(s, AgentCmd::Push));
    }

    #[test]
    fn select_by_short_oid() {
        use crate::git::repo::testutil::TempRepo;
        let t = TempRepo::new();
        t.commit_file("f", "x\n", "hello world");
        let mut repo = crate::git::repo::Repo::open(&t.dir).unwrap();
        let snap = repo.snapshot(10).unwrap();
        let mut app = App::new(Theme::dark(), "test", 1.0, t.dir.clone());
        app.apply(crate::git::ops::Reply::Snapshot(snap));
        let oid = app.snapshot.commits[0].short.clone();
        select_oid(&mut app, &oid).unwrap();
        assert!(matches!(app.selection, Selection::Commit(0)));
    }

    #[test]
    fn status_json_shape() {
        let app = App::new(Theme::dark(), "test", 1.0, PathBuf::from("."));
        let v = status_json(&app);
        assert!(v.get("pid").is_some());
        assert!(v.get("repo").is_some());
    }
}
