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
    /// Whether the UI is focused (reserved; auto-refresh polls regardless).
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
    /// Discard local changes and check out the branch.
    ForceCheckout(String),
    /// Stash, then check out the branch.
    StashAndCheckout {
        branch: String,
        message: String,
    },
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
    PublishGithub {
        name: String,
        description: String,
        private: bool,
    },
    /// Run `git init` in the opened directory.
    InitRepo,
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
            Command::Checkout(_) | Command::ForceCheckout(_) | Command::StashAndCheckout { .. } => {
                "checkout"
            }
            Command::CreateBranch { .. } => "new branch",
            Command::DeleteBranch(_) => "delete branch",
            Command::StashPush { .. } => "stash",
            Command::StashPop(_) => "stash pop",
            Command::StashDrop(_) => "stash drop",
            Command::Fetch => "fetch",
            Command::Pull => "pull",
            Command::Push => "push",
            Command::PublishGithub { .. } => "publish",
            Command::InitRepo => "init",
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
    /// Opened path is not inside a git repository yet.
    NoRepo(PathBuf),
}

pub struct Worker {
    pub tx: mpsc::Sender<Command>,
}

/// Start the worker. Every reply is handed to `reply`, which the runtime
/// uses to forward into its own event channel.
pub fn spawn(path: PathBuf, reply: impl Fn(Reply) + Send + 'static) -> Worker {
    let (tx, rx) = mpsc::channel::<Command>();
    std::thread::Builder::new()
        .name("git".into())
        .spawn(move || {
            let mut repo: Option<Repo> = match Repo::open(&path) {
                Ok(r) => Some(r),
                Err(GitError::NotARepository(_)) => {
                    reply(Reply::NoRepo(path.clone()));
                    None
                }
                Err(e) => {
                    reply(Reply::Error(e.to_string()));
                    None
                }
            };
            let workdir = repo
                .as_ref()
                .map(|r| r.workdir().to_path_buf())
                .unwrap_or_else(|| path.clone());
            let mut limit = COMMIT_LIMIT;
            let (mut stamp, mut work_fp) = repo
                .as_ref()
                .map(refresh_tracking)
                .unwrap_or_default();
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
            if let Some(r) = repo.as_mut() {
                send_snapshot(r, limit);
            }
            loop {
                match rx.recv_timeout(POLL_INTERVAL) {
                    Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(Command::InitRepo) => {
                        if repo.is_some() {
                            reply(Reply::Op {
                                label: "init",
                                result: Ok("already a git repository".into()),
                            });
                            continue;
                        }
                        match Repo::init(&path) {
                            Ok(r) => {
                                repo = Some(r);
                                limit = COMMIT_LIMIT;
                                (stamp, work_fp) =
                                    refresh_tracking(repo.as_ref().expect("just opened"));
                                send_snapshot(repo.as_mut().expect("just opened"), limit);
                                reply(Reply::Op {
                                    label: "init",
                                    result: Ok("initialized git repository".into()),
                                });
                            }
                            Err(e) => {
                                reply(Reply::Op {
                                    label: "init",
                                    result: Err(e.to_string()),
                                });
                            }
                        }
                    }
                    Ok(Command::Refresh) if repo.is_some() => {
                        let r = repo.as_mut().expect("checked");
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(Command::LoadMore(n)) if repo.is_some() => {
                        limit = n;
                        let r = repo.as_mut().expect("checked");
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(Command::LoadDiff(target)) if repo.is_some() => {
                        reply(Reply::Diff(
                            repo.as_ref().expect("checked").diff(&target),
                        ));
                    }
                    Ok(Command::LoadCommitFiles(oid)) if repo.is_some() => {
                        reply(Reply::CommitFiles(
                            oid,
                            repo.as_ref().expect("checked").commit_files(oid),
                        ));
                    }
                    Ok(Command::Focus(_)) => {}
                    Ok(Command::CommitAndPush { message, amend }) if repo.is_some() => {
                        let r = repo.as_mut().expect("checked");
                        let commit_result = write_op(
                            r,
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
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(cmd @ (Command::Fetch | Command::Pull | Command::Push)) if repo.is_some() => {
                        let label = cmd.label();
                        reply(Reply::NetStart(label));
                        let args: &[&str] = match cmd {
                            Command::Fetch => &["fetch", "--all", "--prune"],
                            Command::Pull => &["pull"],
                            _ => &["push"],
                        };
                        let result = run_git_cli(&workdir, args, &reply);
                        reply(Reply::Op { label, result });
                        let r = repo.as_mut().expect("checked");
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(Command::PublishGithub {
                        name,
                        description,
                        private,
                    }) if repo.is_some() => {
                        reply(Reply::NetStart("publish"));
                        let args = gh_repo_create_args(&name, &description, private);
                        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                        let result = run_gh_cli(&workdir, &arg_refs, &reply);
                        reply(Reply::Op {
                            label: "publish",
                            result,
                        });
                        let r = repo.as_mut().expect("checked");
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(cmd) if repo.is_some() => {
                        let r = repo.as_mut().expect("checked");
                        let label = cmd.label();
                        let result = write_op(r, cmd).map_err(|e| e.to_string());
                        reply(Reply::Op { label, result });
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(_) => reply(Reply::Error("not a git repository".into())),
                    Err(mpsc::RecvTimeoutError::Timeout) if repo.is_some() => {
                        let r = repo.as_mut().expect("checked");
                        let (new_stamp, new_fp) = refresh_tracking(r);
                        if new_stamp != stamp || new_fp != work_fp {
                            stamp = new_stamp;
                            work_fp = new_fp;
                            send_snapshot(r, limit);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        })
        .expect("spawn git worker");
    Worker { tx }
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
            let local = repo.checkout(&name, false)?;
            format!("checked out {local}")
        }
        Command::ForceCheckout(name) => {
            let local = repo.checkout(&name, true)?;
            format!("checked out {local} (discarded local changes)")
        }
        Command::StashAndCheckout { branch, message } => {
            let local = repo.stash_and_checkout(&message, &branch)?;
            format!("stashed and checked out {local}")
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

/// Build `gh repo create` arguments for publishing a local repo.
pub fn gh_repo_create_args(name: &str, description: &str, private: bool) -> Vec<String> {
    let mut args = vec![
        "repo".into(),
        "create".into(),
        name.trim().to_owned(),
        if private {
            "--private".into()
        } else {
            "--public".into()
        },
        "--source=.".into(),
        "--remote=origin".into(),
        "--push".into(),
    ];
    let desc = description.trim();
    if !desc.is_empty() {
        args.push("--description".into());
        args.push(desc.to_owned());
    }
    args
}

/// Run `gh <args>` in the working directory, streaming output lines.
fn run_gh_cli(
    workdir: &Path,
    args: &[&str],
    reply: &(impl Fn(Reply) + Send + 'static),
) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command as Proc, Stdio};
    let mut child = Proc::new("gh")
        .args(args)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run gh: {e} (install GitHub CLI and run gh auth login)"))?;
    reply(Reply::NetLine(format!("$ gh {}", args.join(" "))));
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
        Ok("published to GitHub".into())
    } else {
        Err(if last.is_empty() {
            format!("gh {} failed ({status})", args.join(" "))
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

fn refresh_tracking(repo: &Repo) -> (Vec<Option<SystemTime>>, u64) {
    (
        stamp(&repo.watch_paths()),
        repo.worktree_fingerprint().unwrap_or(0),
    )
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
        });
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
        });
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
    fn worker_poll_refreshes_on_worktree_change() {
        use std::time::Instant;

        let t = TempRepo::new();
        t.commit_file("a.txt", "x\n", "init");
        let (tx, rx) = mpsc::channel();
        let w = spawn(t.dir.clone(), move |r| {
            let _ = tx.send(r);
        });
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Snapshot(s) => assert!(s.unstaged.is_empty()),
            other => panic!("unexpected {other:?}"),
        }
        t.write("a.txt", "changed\n");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Reply::Snapshot(s)) if !s.unstaged.is_empty() => {
                    got = true;
                    break;
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(got, "poll should refresh snapshot after worktree edit");
        w.tx.send(Command::Quit).unwrap();
    }

    #[test]
    fn worker_no_repo_sends_norepo() {
        let dir = std::env::temp_dir().join(format!("gitgui-ops-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let w = spawn(dir.clone(), move |r| {
            let _ = tx.send(r);
        });
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::NoRepo(p) => assert_eq!(p, dir),
            other => panic!("unexpected {other:?}"),
        }
        w.tx.send(Command::InitRepo).unwrap();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Snapshot(s) => assert!(s.commits.is_empty()),
            other => panic!("expected snapshot after init, got {other:?}"),
        }
        w.tx.send(Command::Quit).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gh_repo_create_args_include_visibility_and_push() {
        let public = gh_repo_create_args("my-app", "hello", false);
        assert!(public.contains(&"my-app".to_string()));
        assert!(public.contains(&"--public".to_string()));
        assert!(public.contains(&"--push".to_string()));
        assert!(public.contains(&"--description".to_string()));
        let private = gh_repo_create_args("user/my-app", "", true);
        assert!(private.contains(&"--private".to_string()));
        assert!(!private.contains(&"--description".to_string()));
    }
}
