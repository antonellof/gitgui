//! The git worker thread: executes [`Command`]s and replies with
//! [`Reply`]s. The UI thread never touches git2.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use git2::Oid;

use super::actions::{ConflictSide, MergeOutcome, ResetKind};
use super::rebase::{self, TodoAction};
use super::repo::{DiffOpts, DiffTarget, DiffText, FileStatus, GitError, Repo, RepoSnapshot};

pub const COMMIT_LIMIT: usize = 2000;
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// What to do with an in-progress merge, rebase, cherry-pick or revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateAction {
    Continue,
    Abort,
    Skip,
}

impl StateAction {
    pub fn flag(self) -> &'static str {
        match self {
            StateAction::Continue => "--continue",
            StateAction::Abort => "--abort",
            StateAction::Skip => "--skip",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Refresh,
    /// Raise the commit cap (load more).
    LoadMore(usize),
    LoadDiff(DiffTarget),
    LoadCommitFiles(Oid),
    /// Context lines and whitespace handling for every diff from now on.
    SetDiffOpts(DiffOpts),
    /// Whether the UI is focused (reserved; auto-refresh polls regardless).
    Focus(bool),
    Stage(Vec<String>),
    Unstage(Vec<String>),
    StageAll,
    UnstageAll,
    Discard(Vec<String>),
    /// Reset the index and working tree to HEAD, delete untracked files.
    DiscardAll,
    StageHunk {
        path: String,
        hunk_index: usize,
    },
    UnstageHunk {
        path: String,
        hunk_index: usize,
    },
    DiscardHunk {
        path: String,
        hunk_index: usize,
    },
    /// Line indices are into the hunk's `lines` of the unstaged diff.
    StageLines {
        path: String,
        hunk_index: usize,
        lines: Vec<usize>,
    },
    UnstageLines {
        path: String,
        hunk_index: usize,
        lines: Vec<usize>,
    },
    DiscardLines {
        path: String,
        hunk_index: usize,
        lines: Vec<usize>,
    },
    /// Append a pattern to .gitignore.
    Ignore(String),
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
    /// Check out a commit (or tag target) as a detached HEAD.
    CheckoutDetached(Oid),
    CreateBranch {
        name: String,
        from: Oid,
        checkout: bool,
    },
    DeleteBranch(String),
    RenameBranch {
        old: String,
        new: String,
    },
    /// `None` clears the upstream.
    SetUpstream {
        branch: String,
        upstream: Option<String>,
    },
    /// Move a local branch to its upstream when it is only behind.
    FastForward(String),
    /// Merge a branch into the checked out one.
    Merge(String),
    /// `git rebase <onto>` of the checked out branch.
    Rebase(String),
    CherryPick(Oid),
    Revert(Oid),
    Reset {
        oid: Oid,
        kind: ResetKind,
    },
    /// Rewrite history below HEAD with a non-interactive `git rebase -i`.
    RewriteCommit {
        oid: Oid,
        action: TodoAction,
        /// New message for `Reword`.
        message: Option<String>,
        /// Whether `oid` is the root commit (rebase `--root`).
        is_root: bool,
    },
    /// `git rebase -i --autosquash` from the parent of `oid`.
    Autosquash {
        oid: Oid,
        is_root: bool,
    },
    /// Continue, abort or skip the in-progress operation.
    State {
        action: StateAction,
        subcommand: &'static str,
    },
    /// Resolve a conflicted file by taking one side.
    Resolve {
        path: String,
        side: ConflictSide,
    },
    CreateTag {
        name: String,
        oid: Oid,
        message: String,
    },
    DeleteTag(String),
    /// `git push <remote> <tag>`.
    PushTag {
        remote: String,
        tag: String,
    },
    StashPushOpts {
        message: String,
        keep_index: bool,
        include_untracked: bool,
    },
    StashPop(usize),
    StashApply(usize),
    StashDrop(usize),
    BranchFromStash {
        index: usize,
        name: String,
    },
    RemoteAdd {
        name: String,
        url: String,
    },
    RemoteRemove(String),
    RemoteRename {
        old: String,
        new: String,
    },
    RemoteSetUrl {
        name: String,
        url: String,
    },
    Fetch,
    /// `git fetch <remote> --prune`.
    FetchRemote(String),
    Pull,
    /// `git pull --rebase`.
    PullRebase,
    /// `git push`, adding `-u origin <branch>` when the branch has no upstream.
    Push,
    /// `git push --force-with-lease`.
    ForcePush,
    /// `git push <remote> --delete <branch>`.
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
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
            | Command::SetDiffOpts(_)
            | Command::Focus(_)
            | Command::Quit => "",
            Command::Stage(_)
            | Command::StageAll
            | Command::StageHunk { .. }
            | Command::StageLines { .. } => "stage",
            Command::Unstage(_)
            | Command::UnstageAll
            | Command::UnstageHunk { .. }
            | Command::UnstageLines { .. } => "unstage",
            Command::Discard(_)
            | Command::DiscardAll
            | Command::DiscardHunk { .. }
            | Command::DiscardLines { .. } => "discard",
            Command::Ignore(_) => "ignore",
            Command::Commit { .. } | Command::CommitAndPush { .. } => "commit",
            Command::Checkout(_)
            | Command::ForceCheckout(_)
            | Command::StashAndCheckout { .. }
            | Command::CheckoutDetached(_) => "checkout",
            Command::CreateBranch { .. } => "new branch",
            Command::DeleteBranch(_) => "delete branch",
            Command::RenameBranch { .. } => "rename branch",
            Command::SetUpstream { .. } => "upstream",
            Command::FastForward(_) => "fast-forward",
            Command::Merge(_) => "merge",
            Command::Rebase(_) => "rebase",
            Command::CherryPick(_) => "cherry-pick",
            Command::Revert(_) => "revert",
            Command::Reset { .. } => "reset",
            Command::RewriteCommit { action, .. } => match action {
                TodoAction::Drop => "drop commit",
                TodoAction::Squash => "squash",
                TodoAction::Fixup => "fixup",
                TodoAction::Reword => "reword",
                TodoAction::Edit => "edit commit",
                TodoAction::MoveUp | TodoAction::MoveDown => "move commit",
                TodoAction::Keep => "rebase",
            },
            Command::Autosquash { .. } => "autosquash",
            Command::State { action, .. } => match action {
                StateAction::Continue => "continue",
                StateAction::Abort => "abort",
                StateAction::Skip => "skip",
            },
            Command::Resolve { .. } => "resolve",
            Command::CreateTag { .. } => "new tag",
            Command::DeleteTag(_) => "delete tag",
            Command::PushTag { .. } => "push tag",
            Command::StashPushOpts { .. } => "stash",
            Command::StashPop(_) => "stash pop",
            Command::StashApply(_) => "stash apply",
            Command::StashDrop(_) => "stash drop",
            Command::BranchFromStash { .. } => "branch from stash",
            Command::RemoteAdd { .. } => "add remote",
            Command::RemoteRemove(_) => "remove remote",
            Command::RemoteRename { .. } => "rename remote",
            Command::RemoteSetUrl { .. } => "remote url",
            Command::Fetch | Command::FetchRemote(_) => "fetch",
            Command::Pull | Command::PullRebase => "pull",
            Command::Push | Command::ForcePush => "push",
            Command::DeleteRemoteBranch { .. } => "delete remote branch",
            Command::PublishGithub { .. } => "publish",
            Command::InitRepo => "init",
        }
    }

    /// True for operations that run the git CLI and stream output.
    pub fn is_network(&self) -> bool {
        matches!(
            self,
            Command::Fetch
                | Command::FetchRemote(_)
                | Command::Pull
                | Command::PullRebase
                | Command::Push
                | Command::ForcePush
                | Command::PushTag { .. }
                | Command::DeleteRemoteBranch { .. }
                | Command::PublishGithub { .. }
                | Command::Rebase(_)
                | Command::RewriteCommit { .. }
                | Command::Autosquash { .. }
                | Command::State { .. }
        )
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
            let mut diff_opts = DiffOpts::default();
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
                    Ok(Command::SetDiffOpts(o)) => diff_opts = o,
                    Ok(Command::LoadDiff(target)) if repo.is_some() => {
                        reply(Reply::Diff(
                            repo.as_ref().expect("checked").diff(&target, diff_opts),
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
                            diff_opts,
                        )
                        .map_err(|e| e.to_string());
                        let commit_ok = commit_result.is_ok();
                        reply(Reply::Op {
                            label: "commit",
                            result: commit_result,
                        });
                        if commit_ok {
                            reply(Reply::NetStart("push"));
                            let args = push_args(r, false);
                            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                            let push_result = run_git_cli(&workdir, &arg_refs, &[], &reply);
                            reply(Reply::Op {
                                label: "push",
                                result: push_result,
                            });
                        }
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
                    Ok(cmd) if cmd.is_network() && repo.is_some() => {
                        let r = repo.as_mut().expect("checked");
                        let label = cmd.label();
                        reply(Reply::NetStart(label));
                        let (args, envs) = cli_args(r, &cmd);
                        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                        let result = run_git_cli(&workdir, &arg_refs, &envs, &reply);
                        reply(Reply::Op { label, result });
                        send_snapshot(r, limit);
                        (stamp, work_fp) = refresh_tracking(r);
                    }
                    Ok(cmd) if repo.is_some() => {
                        let r = repo.as_mut().expect("checked");
                        let label = cmd.label();
                        let result = write_op(r, cmd, diff_opts).map_err(|e| e.to_string());
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

/// `git push` arguments: plain when the branch has an upstream, otherwise
/// `-u origin <branch>` when an origin exists.
fn push_args(repo: &Repo, force: bool) -> Vec<String> {
    let mut args = vec!["push".to_owned()];
    if force {
        args.push("--force-with-lease".into());
    }
    if let Some((branch, None)) = repo.current_branch_upstream() {
        let remote = if repo.remote_url("origin").is_some() {
            Some("origin".to_owned())
        } else {
            None
        };
        if let Some(remote) = remote {
            args.push("-u".into());
            args.push(remote);
            args.push(branch);
        }
    }
    args
}

/// Arguments and environment for the git CLI commands.
fn cli_args(repo: &Repo, cmd: &Command) -> (Vec<String>, Vec<(String, String)>) {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "gitgui".into());
    let editors = |extra: Vec<(String, String)>| {
        let mut envs = vec![
            (
                "GIT_SEQUENCE_EDITOR".to_owned(),
                format!("{} --sequence-editor", rebase::shell_quote(&exe)),
            ),
            (
                "GIT_EDITOR".to_owned(),
                format!("{} --commit-editor", rebase::shell_quote(&exe)),
            ),
        ];
        envs.extend(extra);
        envs
    };
    match cmd {
        Command::Fetch => (s(&["fetch", "--all", "--prune"]), vec![]),
        Command::FetchRemote(r) => (s(&["fetch", r, "--prune"]), vec![]),
        Command::Pull => (s(&["pull"]), vec![]),
        Command::PullRebase => (
            s(&["pull", "--rebase"]),
            vec![("GIT_EDITOR".to_owned(), "true".to_owned())],
        ),
        Command::Push => (push_args(repo, false), vec![]),
        Command::ForcePush => (push_args(repo, true), vec![]),
        Command::PushTag { remote, tag } => (s(&["push", remote, tag]), vec![]),
        Command::DeleteRemoteBranch { remote, branch } => {
            (s(&["push", remote, "--delete", branch]), vec![])
        }
        Command::Rebase(onto) => (
            s(&["rebase", onto]),
            vec![("GIT_EDITOR".to_owned(), "true".to_owned())],
        ),
        Command::RewriteCommit {
            oid,
            action,
            message,
            is_root,
        } => {
            let base = rewrite_base(*oid, *action, *is_root);
            let mut args = s(&["rebase", "-i"]);
            args.extend(base);
            let mut extra = vec![
                (rebase::ENV_ACTION.to_owned(), action.as_str().to_owned()),
                (rebase::ENV_OID.to_owned(), oid.to_string()),
            ];
            if let Some(m) = message {
                extra.push((rebase::ENV_MESSAGE.to_owned(), m.clone()));
            }
            (args, editors(extra))
        }
        Command::Autosquash { oid, is_root } => {
            let mut args = s(&["rebase", "-i", "--autosquash"]);
            args.extend(rewrite_base(*oid, TodoAction::Keep, *is_root));
            (
                args,
                editors(vec![(
                    rebase::ENV_ACTION.to_owned(),
                    TodoAction::Keep.as_str().to_owned(),
                )]),
            )
        }
        Command::State { action, subcommand } => (
            s(&[subcommand, action.flag()]),
            vec![("GIT_EDITOR".to_owned(), "true".to_owned())],
        ),
        _ => (vec![], vec![]),
    }
}

/// The rebase base for rewriting `oid`: its parent, or the parent of the
/// commit below when the action involves that commit too.
pub fn rewrite_base(oid: Oid, action: TodoAction, is_root: bool) -> Vec<String> {
    let depth = match action {
        TodoAction::Squash | TodoAction::Fixup | TodoAction::MoveDown => 2,
        _ => 1,
    };
    if is_root {
        vec!["--root".into()]
    } else {
        vec![format!("{oid}~{depth}")]
    }
}

fn write_op(repo: &mut Repo, cmd: Command, diff_opts: DiffOpts) -> Result<String, GitError> {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    Ok(match cmd {
        Command::Stage(paths) => {
            let n = paths.len();
            repo.stage(&paths)?;
            format!("staged {n} file{}", plural(n))
        }
        Command::Unstage(paths) => {
            let n = paths.len();
            repo.unstage(&paths)?;
            format!("unstaged {n} file{}", plural(n))
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
            format!("discarded {n} file{}", plural(n))
        }
        Command::DiscardAll => {
            repo.discard_all()?;
            "discarded all changes".into()
        }
        Command::StageHunk { path, hunk_index } => {
            repo.stage_hunk(&path, hunk_index, diff_opts)?;
            format!("staged hunk {} of {path}", hunk_index + 1)
        }
        Command::UnstageHunk { path, hunk_index } => {
            repo.unstage_hunk(&path, hunk_index, diff_opts)?;
            format!("unstaged hunk {} of {path}", hunk_index + 1)
        }
        Command::DiscardHunk { path, hunk_index } => {
            repo.discard_hunk(&path, hunk_index, diff_opts)?;
            format!("discarded hunk {} of {path}", hunk_index + 1)
        }
        Command::StageLines {
            path,
            hunk_index,
            lines,
        } => {
            let n = lines.len();
            repo.stage_lines(&path, hunk_index, &lines, diff_opts)?;
            format!("staged {n} line{} of {path}", plural(n))
        }
        Command::UnstageLines {
            path,
            hunk_index,
            lines,
        } => {
            let n = lines.len();
            repo.unstage_lines(&path, hunk_index, &lines, diff_opts)?;
            format!("unstaged {n} line{} of {path}", plural(n))
        }
        Command::DiscardLines {
            path,
            hunk_index,
            lines,
        } => {
            let n = lines.len();
            repo.discard_lines(&path, hunk_index, &lines, diff_opts)?;
            format!("discarded {n} line{} of {path}", plural(n))
        }
        Command::Ignore(pattern) => {
            repo.ignore(&pattern)?;
            format!("added {pattern} to .gitignore")
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
        Command::CheckoutDetached(oid) => {
            repo.checkout_detached(oid)?;
            format!("checked out {} (detached HEAD)", super::repo::short_id(oid))
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
        Command::RenameBranch { old, new } => {
            repo.rename_branch(&old, &new)?;
            format!("renamed {old} to {new}")
        }
        Command::SetUpstream { branch, upstream } => {
            repo.set_upstream(&branch, upstream.as_deref())?;
            match upstream {
                Some(u) => format!("{branch} now tracks {u}"),
                None => format!("{branch} no longer tracks an upstream"),
            }
        }
        Command::FastForward(name) => match repo.fast_forward(&name)? {
            0 => format!("{name} is up to date"),
            n => format!("fast-forwarded {name} by {n} commit{}", plural(n)),
        },
        Command::Merge(name) => match repo.merge(&name)? {
            MergeOutcome::UpToDate => "already up to date".into(),
            MergeOutcome::FastForward => format!("fast-forwarded to {name}"),
            MergeOutcome::Committed(oid) => {
                format!("merged {name} as {}", super::repo::short_id(oid))
            }
            MergeOutcome::Conflicts(n) => {
                return Err(git2::Error::from_str(&format!(
                    "merge stopped: {n} conflicted file{}, resolve then continue",
                    plural(n)
                ))
                .into())
            }
        },
        Command::CherryPick(oid) => match repo.cherry_pick(oid)? {
            MergeOutcome::Committed(new) => {
                format!(
                    "cherry-picked {} as {}",
                    super::repo::short_id(oid),
                    super::repo::short_id(new)
                )
            }
            MergeOutcome::Conflicts(n) => {
                return Err(git2::Error::from_str(&format!(
                    "cherry-pick stopped: {n} conflicted file{}, resolve then continue",
                    plural(n)
                ))
                .into())
            }
            _ => "nothing to cherry-pick".into(),
        },
        Command::Revert(oid) => match repo.revert(oid)? {
            MergeOutcome::Committed(new) => {
                format!(
                    "reverted {} as {}",
                    super::repo::short_id(oid),
                    super::repo::short_id(new)
                )
            }
            MergeOutcome::Conflicts(n) => {
                return Err(git2::Error::from_str(&format!(
                    "revert stopped: {n} conflicted file{}, resolve then continue",
                    plural(n)
                ))
                .into())
            }
            _ => "nothing to revert".into(),
        },
        Command::Reset { oid, kind } => {
            repo.reset(oid, kind)?;
            format!("reset ({}) to {}", kind.label(), super::repo::short_id(oid))
        }
        Command::Resolve { path, side } => {
            repo.resolve_conflict(&path, side)?;
            format!(
                "resolved {path} with {}",
                match side {
                    ConflictSide::Ours => "ours",
                    ConflictSide::Theirs => "theirs",
                }
            )
        }
        Command::CreateTag { name, oid, message } => {
            repo.create_tag(&name, oid, &message)?;
            format!("tagged {} as {name}", super::repo::short_id(oid))
        }
        Command::DeleteTag(name) => {
            repo.delete_tag(&name)?;
            format!("deleted tag {name}")
        }
        Command::StashPushOpts {
            message,
            keep_index,
            include_untracked,
        } => {
            repo.stash_push_opts(&message, keep_index, include_untracked)?;
            "stashed changes".into()
        }
        Command::StashPop(i) => {
            repo.stash_pop(i)?;
            "stash applied and dropped".into()
        }
        Command::StashApply(i) => {
            repo.stash_apply(i)?;
            "stash applied".into()
        }
        Command::StashDrop(i) => {
            repo.stash_drop(i)?;
            "stash dropped".into()
        }
        Command::BranchFromStash { index, name } => {
            repo.branch_from_stash(index, &name)?;
            format!("created {name} from stash")
        }
        Command::RemoteAdd { name, url } => {
            repo.remote_add(&name, &url)?;
            format!("added remote {name}")
        }
        Command::RemoteRemove(name) => {
            repo.remote_remove(&name)?;
            format!("removed remote {name}")
        }
        Command::RemoteRename { old, new } => {
            repo.remote_rename(&old, &new)?;
            format!("renamed remote {old} to {new}")
        }
        Command::RemoteSetUrl { name, url } => {
            repo.remote_set_url(&name, &url)?;
            format!("updated url of {name}")
        }
        other => format!("{other:?}"),
    })
}

/// Run `git <args>` in the working directory, streaming output lines.
/// Never prompts: GIT_TERMINAL_PROMPT=0 makes credential failures fail fast.
fn run_git_cli(
    workdir: &Path,
    args: &[&str],
    envs: &[(String, String)],
    reply: &(impl Fn(Reply) + Send + 'static),
) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command as Proc, Stdio};
    let mut child = Proc::new("git")
        .args(args)
        .current_dir(workdir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
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
        Ok(format!("{} done", args.join(" ")))
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
    fn rewrite_commands_build_rebase_invocations() {
        let t = TempRepo::new();
        t.commit_file("a", "1", "one");
        let oid = t.commit_file("a", "2", "two");
        let r = Repo::open(&t.dir).unwrap();
        let (args, envs) = cli_args(
            &r,
            &Command::RewriteCommit {
                oid,
                action: TodoAction::Reword,
                message: Some("new".into()),
                is_root: false,
            },
        );
        assert_eq!(args, vec!["rebase", "-i", &format!("{oid}~1")]);
        let env = |k: &str| envs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert!(env("GIT_SEQUENCE_EDITOR").unwrap().ends_with("--sequence-editor"));
        assert!(env("GIT_EDITOR").unwrap().ends_with("--commit-editor"));
        assert_eq!(env(rebase::ENV_ACTION).as_deref(), Some("reword"));
        assert_eq!(env(rebase::ENV_OID).as_deref(), Some(oid.to_string().as_str()));
        assert_eq!(env(rebase::ENV_MESSAGE).as_deref(), Some("new"));
        let (args, _) = cli_args(
            &r,
            &Command::RewriteCommit {
                oid,
                action: TodoAction::Squash,
                message: None,
                is_root: false,
            },
        );
        assert_eq!(args[2], format!("{oid}~2"), "squash rebases from below the target");
        let (args, _) = cli_args(
            &r,
            &Command::RewriteCommit {
                oid,
                action: TodoAction::Drop,
                message: None,
                is_root: true,
            },
        );
        assert_eq!(args, vec!["rebase", "-i", "--root"]);
        let (args, _) = cli_args(
            &r,
            &Command::Autosquash {
                oid,
                is_root: false,
            },
        );
        assert_eq!(args[..3], ["rebase", "-i", "--autosquash"]);
        let (args, envs) = cli_args(
            &r,
            &Command::State {
                action: StateAction::Abort,
                subcommand: "rebase",
            },
        );
        assert_eq!(args, vec!["rebase", "--abort"]);
        assert!(envs.iter().any(|(k, v)| k == "GIT_EDITOR" && v == "true"));
        assert_eq!(
            cli_args(
                &r,
                &Command::DeleteRemoteBranch {
                    remote: "origin".into(),
                    branch: "old".into()
                }
            )
            .0,
            vec!["push", "origin", "--delete", "old"]
        );
        // Push without an upstream and with an origin sets the upstream.
        assert_eq!(push_args(&r, false), vec!["push"]);
        r.remote_add("origin", "https://example.invalid/x.git").unwrap();
        let branch = r.head_info().unwrap().branch_name.unwrap();
        assert_eq!(push_args(&r, true), vec!["push", "--force-with-lease", "-u", "origin", &branch]);
    }

    #[test]
    fn rebase_onto_branch_and_abort_through_cli() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "a\n", "base");
        let base = t.repo.head().unwrap().target().unwrap();
        let main = t.repo.head().unwrap().shorthand().unwrap().to_owned();
        let r0 = Repo::open(&t.dir).unwrap();
        r0.create_branch("topic", base, true).unwrap();
        t.commit_file("t.txt", "t\n", "topic work");
        r0.checkout(&main, false).unwrap();
        t.commit_file("m.txt", "m\n", "main work");
        r0.checkout("topic", false).unwrap();
        drop(r0);
        let (tx, rx) = mpsc::channel();
        let w = spawn(t.dir.clone(), move |r| {
            let _ = tx.send(r);
        });
        let _ = rx.recv_timeout(Duration::from_secs(5));
        w.tx.send(Command::Rebase(main.clone())).unwrap();
        let mut result = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
                Reply::Op { label: "rebase", result: r } => {
                    result = Some(r);
                }
                Reply::Snapshot(s) if result.is_some() => {
                    assert_eq!(s.commits[0].summary, "topic work");
                    assert_eq!(s.commits[1].summary, "main work");
                    assert_eq!(s.state, crate::git::repo::RepoState::Clean);
                    break;
                }
                _ => {}
            }
        }
        assert!(result.unwrap().is_ok());
        // A conflicting rebase stops; abort restores the branch.
        t.write("a.txt", "topic\n");
        t.add("a.txt");
        t.commit("topic edits a");
        let r = Repo::open(&t.dir).unwrap();
        r.checkout(&main, false).unwrap();
        t.write("a.txt", "main\n");
        t.add("a.txt");
        let main_tip = t.commit("main edits a");
        r.checkout("topic", false).unwrap();
        drop(r);
        w.tx.send(Command::Refresh).unwrap();
        w.tx.send(Command::Rebase(main.clone())).unwrap();
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
                Reply::Op { label: "rebase", result } => assert!(result.is_err()),
                Reply::Snapshot(s) if s.state == crate::git::repo::RepoState::Rebase => {
                    assert_eq!(s.conflicted.len(), 1);
                    assert!(s.rebase_progress.is_some());
                    break;
                }
                _ => {}
            }
        }
        w.tx.send(Command::State {
            action: StateAction::Abort,
            subcommand: "rebase",
        })
        .unwrap();
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).unwrap() {
                Reply::Snapshot(s) if s.state == crate::git::repo::RepoState::Clean => {
                    assert_eq!(s.commits[0].summary, "topic edits a");
                    assert!(s.conflicted.is_empty());
                    assert_ne!(s.commits[0].oid, main_tip);
                    break;
                }
                _ => {}
            }
        }
        w.tx.send(Command::Quit).unwrap();
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
