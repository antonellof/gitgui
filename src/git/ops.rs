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
    Quit,
}

#[derive(Debug)]
pub enum Reply {
    Snapshot(Arc<RepoSnapshot>),
    Diff(Result<DiffText, GitError>),
    CommitFiles(Oid, Result<Vec<FileStatus>, GitError>),
    Error(String),
}

pub struct Worker {
    pub tx: mpsc::Sender<Command>,
}

/// Start the worker. Every reply is handed to `reply`, which the runtime
/// uses to forward into its own event channel.
pub fn spawn(path: PathBuf, reply: impl Fn(Reply) + Send + 'static) -> Result<Worker, GitError> {
    let mut repo = Repo::open(&path)?;
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
        w.tx.send(Command::LoadDiff(DiffTarget::WorkdirUnstaged("a.txt".into()))).unwrap();
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Reply::Diff(Ok(d)) => assert_eq!(d.hunks[0].lines.iter().filter(|l| l.origin == '+').count(), 1),
            other => panic!("unexpected {other:?}"),
        }
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
