//! The git worker thread: executes [`Command`]s and replies with
//! [`Reply`]s. The UI thread never touches git2.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use git2::Oid;

use super::repo::{DiffTarget, DiffText, FileStatus, GitError, Repo, RepoSnapshot};

pub const COMMIT_LIMIT: usize = 2000;
const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Refresh,
    /// Raise the commit cap (load more).
    LoadMore(usize),
    LoadDiff(DiffTarget),
    LoadCommitFiles(Oid),
    /// Whether the UI is focused; polling only happens while focused.
    Focus(bool),
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    Discard(Vec<String>),
    StageHunk {
        path: String,
        hunk_index: usize,
    },
    UnstageHunk {
        path: String,
        hunk_index: usize,
    },
    Commit {
        message: String,
        amend: bool,
    },
    /// Commit, then run `git push` on success.
    CommitAndPush {
        message: String,
        amend: bool,
    },
    Checkout(String),
    CreateBranch {
        name: String,
        from: Oid,
        checkout: bool,
    },
    DeleteBranch(String),
    StashPush {
        message: String,
    },
    StashPop(usize),
    StashDrop(usize),
    Fetch,
    Pull,
    Push,
    Quit,
}

impl Command {
    /// Short label for toasts and the status bar.
    pub fn label(&self) -> &'static str {
        match self {
            Command::Refresh
            | Command::LoadMore(_)
            | Command::LoadDiff(_)
            | Command::LoadCommitFiles(_)
            | Command::Focus(_)
            | Command::Quit => "",
            Command::Stage(_) | Command::StageAll | Command::StageHunk { .. } => "stage",
            Command::Unstage(_) | Command::UnstageAll | Command::UnstageHunk { .. } => "unstage",
            Command::Discard(_) => "discard",
            Command::Commit { .. } | Command::CommitAndPush { .. } => "commit",
            Command::Checkout(_) => "checkout",
            Command::CreateBranch { .. } => "new branch",
            Command::DeleteBranch(_) => "delete branch",
            Command::StashPush { .. } => "stash",
            Command::StashPop(_) => "stash pop",
            Command::StashDrop(_) => "stash drop",
            Command::Fetch => "fetch",
            Command::Pull => "pull",
            Command::Push => "push",
        }
    }
}

#[derive(Debug)]
pub enum Reply {
    Snapshot(Arc<RepoSnapshot>),
    Diff(Result<DiffText, GitError>),
    CommitFiles(Oid, Result<Vec<FileStatus>, GitError>),
    /// A write or network operation finished.
    Op {
        label: &'static str,
        result: Result<String, String>,
    },
    /// One line of git CLI output while a network operation runs.
    NetLine(String),
    /// A network operation started (label) so the UI can open the log.
    NetStart(&'static str),
    Error(String),
}

pub struct Worker {
    pub tx: mpsc::Sender<Command>,
}

/// Start the worker. Every reply is handed to `reply`, which the runtime
/// uses to forward into its own event channel.
pub fn spawn(path: PathBuf, reply: impl Fn(Reply) + Send + 'static) -> Result<Worker, GitError> {
    let mut repo = Repo::open(&path)?;
    let workdir = repo.workdir().to_path_buf();
    let (tx, rx) = mpsc::channel::<Command>();
    std::thread::Builder::new()
        .name("git".into())
        .spawn(move || {
            let mut limit = COMMIT_LIMIT;
            let mut focused = true;
            let mut stamp = stamp(&repo.watch_paths());
            let send_snapshot = |repo: &mut Repo, limit: usize| -> Option<Arc<RepoSnapshot>> {
                match repo.snapshot(limit) {
                    Ok(s) => {
                        reply(Reply::Snapshot(s.clone()));
                        Some(s)
                    }
                    Err(e) => {
                        reply(Reply::Error(e.to_string()));
                        None
                    }
                }
            };
            send_snapshot(&mut repo, limit);
            loop {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(Command::Refresh) => {
                        send_snapshot(&mut repo, limit);
                        stamp = self::stamp(&repo.watch_paths());
                    }
                    Ok(Command::LoadMore(n)) => {
                        limit = n;
                        send_snapshot(&mut repo, limit);
                    }
                    Ok(Command::LoadDiff(target)) => {
                        reply(Reply::Diff(repo.diff(&target)));
                    }
                    Ok(Command::LoadCommitFiles(oid)) => {
                        reply(Reply::CommitFiles(oid, repo.commit_files(oid)));
                    }
                    Ok(Command::Focus(f)) => focused = f,
                    Ok(Command::CommitAndPush { message, amend }) => {
                        let commit_result = write_op(
                            &mut repo,
                            Command::Commit {
                                message,
                                amend,
                            },
                        )
                        .map_err(|e| e.to_string());
                        let commit_ok = commit_result.is_ok();
                        reply(Reply::Op {
                            label: "commit",
                            result: commit_result,
                        });
                        if commit_ok {
                            reply(Reply::NetStart("push"));
                            let push_result = run_git_cli(&workdir, &["push"], &reply);
                            reply(Reply::Op {
                                label: "push",
                                result: push_result,
                            });
                        }
                        send_snapshot(&mut repo, limit);
                        stamp = self::stamp(&repo.watch_paths());
                    }
                    Ok(cmd @ (Command::Fetch | Command::Pull | Command::Push)) => {
                        let label = cmd.label();
                        reply(Reply::NetStart(label));
                        let args: &[&str] = match cmd {
                            Command::Fetch => &["fetch", "--all", "--prune"],
                            Command::Pull => &["pull"],
                            _ => &["push"],
                        };
                        let result = run_git_cli(&workdir, args, &reply);
                        reply(Reply::Op { label, result });
                        send_snapshot(&mut repo, limit);
                        stamp = self::stamp(&repo.watch_paths());
                    }
                    Ok(cmd) => {
                        let label = cmd.label();
                        let result = write_op(&mut repo, cmd).map_err(|e| e.to_string());
                        reply(Reply::Op { label, result });
                        send_snapshot(&mut repo, limit);
                        stamp = self::stamp(&repo.watch_paths());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if focused {
                            let now = self::stamp(&repo.watch_paths());
                            if now != stamp {
                                stamp = now;
                                send_snapshot(&mut repo, limit);
                            }
                        }
                    }
                }
            }
        })
        .expect("spawn git worker");
    Ok(Worker { tx })
}

fn write_op(repo: &mut Repo, cmd: Command) -> Result<String, GitError> {
    Ok(match cmd {
        Command::Stage(paths) => {
            let n = paths.len();
            repo.stage(&paths)?;
            format!("staged {n} file{}", if n == 1 { "" } else { "s" })
        }
        Command::Unstage(paths) => {
            let n = paths.len();
            repo.unstage(&paths)?;
            format!("unstaged {n} file{}", if n == 1 { "" } else { "s" })
        }
        Command::StageAll => {
            repo.stage_all()?;
            "staged everything".into()
        }
        Command::UnstageAll => {
            repo.unstage_all()?;
            "unstaged everything".into()
        }
        Command::Discard(paths) => {
            let n = paths.len();
            repo.discard(&paths)?;
            format!("discarded {n} file{}", if n == 1 { "" } else { "s" })
        }
        Command::StageHunk { path, hunk_index } => {
            repo.stage_hunk(&path, hunk_index)?;
            format!("staged hunk {} of {path}", hunk_index + 1)
        }
        Command::UnstageHunk { path, hunk_index } => {
            repo.unstage_hunk(&path, hunk_index)?;
            format!("unstaged hunk {} of {path}", hunk_index + 1)
        }
        Command::Commit { message, amend } => {
            let oid = repo.commit(&message, amend)?;
            format!(
                "{} {}",
                if amend { "amended" } else { "committed" },
                super::repo::short_id(oid)
            )
        }
        Command::Checkout(name) => {
            let local = repo.checkout(&name)?;
            format!("checked out {local}")
        }
        Command::CreateBranch {
            name,
            from,
            checkout,
        } => {
            repo.create_branch(&name, from, checkout)?;
            format!("created branch {name}")
        }
        Command::DeleteBranch(name) => {
            repo.delete_branch(&name)?;
            format!("deleted branch {name}")
        }
        Command::StashPush { message } => {
            repo.stash_push(&message)?;
            "stashed changes".into()
        }
        Command::StashPop(i) => {
            repo.stash_pop(i)?;
            "stash applied".into()
        }
        Command::StashDrop(i) => {
            repo.stash_drop(i)?;
            "stash dropped".into()
        }
        other => format!("{other:?}"),
    })
}

/// Run `git <args>` in the working directory, streaming output lines.
/// Never prompts: GIT_TERMINAL_PROMPT=0 makes credential failures fail fast.
fn run_git_cli(
    workdir: &Path,
    args: &[&str],
    reply: &(impl Fn(Reply) + Send + 'static),
) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command as Proc, Stdio};
    let mut child = Proc::new("git")
        .args(args)
        .current_dir(workdir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    reply(Reply::NetLine(format!("$ git {}", args.join(" "))));
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel::<String>();
    let tx2 = tx.clone();
    let h1 = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            let _ = tx.send(line);
        }
    });
    let h2 = std::thread::spawn(move || {
        // git writes progress with \r; split on both so the log stays tidy.
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for part in buf.split(|&b| b == b'\r' || b == b'\n') {
                        let s = String::from_utf8_lossy(part).trim_end().to_owned();
                        if !s.is_empty() {
                            let _ = tx2.send(s);
                        }
                    }
                }
            }
        }
    });
    let mut last = String::new();
    for line in rx {
        last = line.clone();
        reply(Reply::NetLine(line));
    }
    let _ = h1.join();
    let _ = h2.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(format!("{} done", args[0]))
    } else {
        Err(if last.is_empty() {
            format!("git {} failed ({status})", args[0])
        } else {
            last
        })
    }
}

/// Modification times of the watched paths. Cheap enough to run every 2 s.
fn stamp(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    paths.iter().map(|p| mtime(p)).collect()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo::testutil::TempRepo;

    #[test]
    fn worker_sends_snapshot_then_diff() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "x\n", "init");
        t.write("a.txt", "x\ny\n");
        let (tx, rx) = mpsc::channel();
        let w = spawn(t.dir.clone(), move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Snapshot(s) => assert_eq!(s.commits.len(), 1),
            other => panic!("unexpected {other:?}"),
        }
        w.tx.send(Command::LoadDiff(DiffTarget::WorkdirUnstaged(
            "a.txt".into(),
        )))
        .unwrap();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Diff(Ok(d)) => assert_eq!(
                d.hunks[0].lines.iter().filter(|l| l.origin == '+').count(),
                1
            ),
            other => panic!("unexpected {other:?}"),
        }
        w.tx.send(Command::StageAll).unwrap();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Op {
                label: "stage",
                result: Ok(_),
            } => {}
            other => panic!("unexpected {other:?}"),
        }
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Snapshot(s) => {
                assert_eq!(s.staged.len(), 1);
                assert!(s.unstaged.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        // A network op against a repo without remotes fails fast and still
        // streams the command line into the log.
        w.tx.send(Command::Push).unwrap();
        let mut saw_start = false;
        let mut saw_line = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
                Reply::NetStart("push") => saw_start = true,
                Reply::NetLine(l) => saw_line |= l.starts_with("$ git push"),
                Reply::Op {
                    label: "push",
                    result,
                } => {
                    assert!(result.is_err());
                    break;
                }
                Reply::Snapshot(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_start && saw_line);
        w.tx.send(Command::Quit).unwrap();
    }

    #[test]
    fn commit_and_push_commits_then_pushes() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "x\n", "init");
        t.write("a.txt", "x\ny\n");
        let (tx, rx) = mpsc::channel();
        let w = spawn(t.dir.clone(), move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(5));
        w.tx.send(Command::StageAll).unwrap();
        while !matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Reply::Snapshot(_))
        ) {}
        w.tx
            .send(Command::CommitAndPush {
                message: "second".into(),
                amend: false,
            })
            .unwrap();
        let mut saw_commit = false;
        let mut saw_push_start = false;
        loop {
            match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
                Reply::Op {
                    label: "commit",
                    result: Ok(_),
                } => saw_commit = true,
                Reply::NetStart("push") => saw_push_start = true,
                Reply::Op {
                    label: "push",
                    result: _,
                } => break,
                Reply::Snapshot(s) if saw_commit && s.commits[0].summary == "second" => {}
                Reply::NetLine(_) | Reply::Snapshot(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_commit && saw_push_start);
        w.tx.send(Command::Quit).unwrap();
    }

    #[test]
    fn worker_fails_outside_repo() {
        let dir = std::env::temp_dir().join(format!("gitgui-ops-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(spawn(dir.clone(), |_| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
