//! Repository access through git2. Everything here runs on the worker thread.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use git2::{Oid, Repository};

use super::graph::{self, GraphLayout};

#[derive(Debug)]
pub enum GitError {
    NotARepository(PathBuf),
    Git(git2::Error),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NotARepository(p) => write!(f, "not a git repository: {}", p.display()),
            GitError::Git(e) => write!(f, "{}", e.message()),
        }
    }
}

impl std::error::Error for GitError {}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self {
        GitError::Git(e)
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadInfo {
    pub branch_name: Option<String>,
    pub oid: Option<Oid>,
    pub detached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub oid: Oid,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub is_head: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub oid: Oid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    pub message: String,
    pub oid: Oid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRow {
    pub oid: Oid,
    pub short: String,
    pub parents: Vec<Oid>,
    pub summary: String,
    /// Message after the first paragraph, trimmed. Empty for one-liners.
    pub body: String,
    pub author: String,
    pub email: String,
    /// Seconds since the epoch, author time.
    pub time: i64,
    pub refs: Vec<RefLabel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    TypeChange,
    Conflicted,
}

impl FileKind {
    pub fn letter(self) -> &'static str {
        match self {
            FileKind::Added => "A",
            FileKind::Modified => "M",
            FileKind::Deleted => "D",
            FileKind::Renamed => "R",
            FileKind::Untracked => "?",
            FileKind::TypeChange => "T",
            FileKind::Conflicted => "!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: FileKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffTarget {
    WorkdirUnstaged(String),
    Staged(String),
    Commit(Oid, String),
}

impl DiffTarget {
    pub fn path(&self) -> &str {
        match self {
            DiffTarget::WorkdirUnstaged(p) | DiffTarget::Staged(p) | DiffTarget::Commit(_, p) => p,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// '+', '-', ' ' or another origin char from libgit2.
    pub origin: char,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
    /// The file has no newline after this line (unified diff marker).
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffText {
    pub target: DiffTarget,
    pub binary: bool,
    pub too_large: bool,
    /// Added, Deleted, Modified or Renamed (Untracked shows as Added).
    pub status: FileKind,
    pub hunks: Vec<Hunk>,
}

/// How diffs are produced: context lines and whitespace handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffOpts {
    pub context: u32,
    pub ignore_whitespace: bool,
}

impl Default for DiffOpts {
    fn default() -> Self {
        DiffOpts {
            context: 3,
            ignore_whitespace: false,
        }
    }
}

impl DiffOpts {
    pub const MAX_CONTEXT: u32 = 20;

    pub fn apply(&self, opts: &mut git2::DiffOptions) {
        opts.context_lines(self.context);
        if self.ignore_whitespace {
            opts.ignore_whitespace(true);
        }
    }
}

/// Files above this size are not diffed.
pub const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

/// An operation the repository is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoState {
    #[default]
    Clean,
    Merge,
    Revert,
    CherryPick,
    Rebase,
    Bisect,
    Other,
}

impl RepoState {
    pub fn label(self) -> &'static str {
        match self {
            RepoState::Clean => "",
            RepoState::Merge => "merge",
            RepoState::Revert => "revert",
            RepoState::CherryPick => "cherry-pick",
            RepoState::Rebase => "rebase",
            RepoState::Bisect => "bisect",
            RepoState::Other => "operation",
        }
    }

    /// The git subcommand that owns `--continue` / `--abort` for this state.
    pub fn git_subcommand(self) -> Option<&'static str> {
        match self {
            RepoState::Merge => Some("merge"),
            RepoState::Revert => Some("revert"),
            RepoState::CherryPick => Some("cherry-pick"),
            RepoState::Rebase => Some("rebase"),
            RepoState::Clean | RepoState::Bisect | RepoState::Other => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RepoSnapshot {
    pub path: PathBuf,
    pub head: Option<HeadInfo>,
    pub branches: Vec<Branch>,
    pub tags: Vec<Tag>,
    pub stashes: Vec<Stash>,
    pub remotes: Vec<String>,
    /// (name, url) for each remote.
    pub remote_urls: Vec<(String, String)>,
    pub commits: Vec<CommitRow>,
    pub graph: GraphLayout,
    /// True when more commits exist beyond the cap.
    pub truncated: bool,
    pub unstaged: Vec<FileStatus>,
    pub staged: Vec<FileStatus>,
    pub conflicted: Vec<FileStatus>,
    pub user_name: String,
    pub user_email: String,
    /// Full message of the HEAD commit, offered when amending.
    pub head_message: Option<String>,
    /// Merge, rebase, cherry-pick or revert in progress.
    pub state: RepoState,
    /// (done, total) while a rebase runs, from .git/rebase-merge.
    pub rebase_progress: Option<(usize, usize)>,
}

impl RepoSnapshot {
    pub fn is_dirty(&self) -> bool {
        !self.unstaged.is_empty() || !self.staged.is_empty() || !self.conflicted.is_empty()
    }
}

pub struct Repo {
    pub(crate) repo: Repository,
    pub(crate) workdir: PathBuf,
}

impl Repo {
    /// Discover the repository containing `path`.
    pub fn open(path: &Path) -> Result<Repo> {
        let repo =
            Repository::discover(path).map_err(|_| GitError::NotARepository(path.to_path_buf()))?;
        let workdir = repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| repo.path().to_path_buf());
        Ok(Repo { repo, workdir })
    }

    /// Create a new repository at `path` (`git init`).
    pub fn init(path: &Path) -> Result<Repo> {
        let repo = Repository::init(path).map_err(GitError::Git)?;
        let workdir = repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf());
        Ok(Repo { repo, workdir })
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Paths whose mtime signals repository or working tree changes.
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        let g = self.repo.path().to_path_buf();
        vec![
            g.join("HEAD"),
            g.join("index"),
            g.join("refs"),
            g.join("packed-refs"),
            g.join("refs/heads"),
            self.workdir.clone(),
        ]
    }

    /// Cheap hash of working tree status and file mtimes for auto-refresh polling.
    pub fn worktree_fingerprint(&self) -> Result<u64> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true)
            .include_ignored(false);
        let statuses = self.repo.statuses(Some(&mut opts))?;
        let wd = self.workdir();
        let mut entries: Vec<_> = statuses.iter().collect();
        entries.sort_by(|a, b| a.path().unwrap_or("").cmp(b.path().unwrap_or("")));
        let mut h = DefaultHasher::new();
        for e in entries {
            e.path().unwrap_or("").hash(&mut h);
            e.status().bits().hash(&mut h);
            if let Ok(path) = e.path() {
                let p = wd.join(path);
                if let Ok(meta) = std::fs::metadata(&p) {
                    if let Ok(t) = meta.modified() {
                        t.hash(&mut h);
                    }
                }
            }
        }
        Ok(h.finish())
    }

    pub fn snapshot(&mut self, commit_limit: usize) -> Result<Arc<RepoSnapshot>> {
        let head = self.head_info();
        let (branches, mut labels) = self.branches(&head)?;
        let tags = self.tags(&mut labels)?;
        if let Some(h) = &head {
            if let (Some(oid), true) = (h.oid, h.detached) {
                labels.push((
                    oid,
                    RefLabel {
                        name: "HEAD".into(),
                        kind: RefKind::Head,
                    },
                ));
            }
        }
        let stashes = self.stashes()?;
        let remotes: Vec<String> = self
            .repo
            .remotes()
            .map(|r| r.iter().flatten().flatten().map(|s| s.to_owned()).collect())
            .unwrap_or_default();
        let remote_urls: Vec<(String, String)> = remotes
            .iter()
            .filter_map(|n: &String| self.remote_url(n).map(|u| (n.clone(), u)))
            .collect();
        let (commits, truncated) = self.log(&branches, &head, &labels, commit_limit)?;
        let graph = graph::layout(&commits);
        let (unstaged, staged, conflicted) = self.status()?;
        let cfg = self.repo.config().ok();
        let user_name = cfg
            .as_ref()
            .and_then(|c| c.get_string("user.name").ok())
            .unwrap_or_default();
        let user_email = cfg
            .as_ref()
            .and_then(|c| c.get_string("user.email").ok())
            .unwrap_or_default();
        Ok(Arc::new(RepoSnapshot {
            path: self.workdir.clone(),
            head,
            branches,
            tags,
            stashes,
            remotes,
            remote_urls,
            commits,
            graph,
            truncated,
            unstaged,
            staged,
            conflicted,
            user_name,
            user_email,
            head_message: self.head_message(),
            state: self.state(),
            rebase_progress: self.rebase_progress(),
        }))
    }

    /// The operation in progress, if any.
    pub fn state(&self) -> RepoState {
        use git2::RepositoryState as S;
        match self.repo.state() {
            S::Clean => RepoState::Clean,
            S::Merge => RepoState::Merge,
            S::Revert | S::RevertSequence => RepoState::Revert,
            S::CherryPick | S::CherryPickSequence => RepoState::CherryPick,
            S::Rebase | S::RebaseInteractive | S::RebaseMerge => RepoState::Rebase,
            S::Bisect => RepoState::Bisect,
            S::ApplyMailbox | S::ApplyMailboxOrRebase => RepoState::Other,
        }
    }

    fn rebase_progress(&self) -> Option<(usize, usize)> {
        let dir = self.repo.path().join("rebase-merge");
        let read = |name: &str| -> Option<usize> {
            std::fs::read_to_string(dir.join(name))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        Some((read("msgnum")?, read("end")?))
    }

    pub(crate) fn head_info(&self) -> Option<HeadInfo> {
        let head = self.repo.head().ok();
        let detached = self.repo.head_detached().unwrap_or(false);
        match head {
            Some(r) => Some(HeadInfo {
                branch_name: if detached {
                    None
                } else {
                    r.shorthand().ok().map(|s| s.to_owned())
                },
                oid: r.target(),
                detached,
            }),
            None => {
                // Unborn branch: HEAD points to a ref that does not exist yet.
                let name = std::fs::read_to_string(self.repo.path().join("HEAD"))
                    .ok()
                    .and_then(|s| {
                        s.trim()
                            .strip_prefix("ref: refs/heads/")
                            .map(|b| b.to_owned())
                    });
                Some(HeadInfo {
                    branch_name: name,
                    oid: None,
                    detached: false,
                })
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn branches(&self, head: &Option<HeadInfo>) -> Result<(Vec<Branch>, Vec<(Oid, RefLabel)>)> {
        let mut out = Vec::new();
        let mut labels = Vec::new();
        let head_name = head.as_ref().and_then(|h| h.branch_name.clone());
        for item in self.repo.branches(None)? {
            let (branch, kind) = item?;
            let Some(name) = branch.name()?.map(|s| s.to_owned()) else {
                continue;
            };
            let Some(oid) = branch.get().target() else {
                continue;
            };
            let is_remote = kind == git2::BranchType::Remote;
            if is_remote && name.ends_with("/HEAD") {
                continue;
            }
            let (upstream, ahead, behind) = if is_remote {
                (None, 0, 0)
            } else {
                match branch.upstream() {
                    Ok(up) => {
                        let up_name = up.name()?.map(|s| s.to_owned());
                        let (a, b) = match up.get().target() {
                            Some(uoid) => self.repo.graph_ahead_behind(oid, uoid).unwrap_or((0, 0)),
                            None => (0, 0),
                        };
                        (up_name, a, b)
                    }
                    Err(_) => (None, 0, 0),
                }
            };
            let is_head = !is_remote && head_name.as_deref() == Some(name.as_str());
            labels.push((
                oid,
                RefLabel {
                    name: name.clone(),
                    kind: if is_remote {
                        RefKind::RemoteBranch
                    } else {
                        RefKind::LocalBranch
                    },
                },
            ));
            out.push(Branch {
                name,
                oid,
                is_remote,
                upstream,
                ahead,
                behind,
                is_head,
            });
        }
        out.sort_by(|a, b| {
            a.is_remote
                .cmp(&b.is_remote)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok((out, labels))
    }

    fn tags(&self, labels: &mut Vec<(Oid, RefLabel)>) -> Result<Vec<Tag>> {
        let mut out = Vec::new();
        self.repo.tag_foreach(|oid, name| {
            let name = String::from_utf8_lossy(name);
            let short = name.strip_prefix("refs/tags/").unwrap_or(&name).to_owned();
            // Peel annotated tags to the commit they point at.
            let target = self
                .repo
                .find_object(oid, None)
                .ok()
                .and_then(|o| o.peel(git2::ObjectType::Commit).ok())
                .map(|c| c.id())
                .unwrap_or(oid);
            labels.push((
                target,
                RefLabel {
                    name: short.clone(),
                    kind: RefKind::Tag,
                },
            ));
            out.push(Tag {
                name: short,
                oid: target,
            });
            true
        })?;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn stashes(&mut self) -> Result<Vec<Stash>> {
        let mut out = Vec::new();
        self.repo.stash_foreach(|index, message, oid| {
            out.push(Stash {
                index,
                message: message.to_owned(),
                oid: *oid,
            });
            true
        })?;
        Ok(out)
    }

    fn log(
        &self,
        branches: &[Branch],
        head: &Option<HeadInfo>,
        labels: &[(Oid, RefLabel)],
        limit: usize,
    ) -> Result<(Vec<CommitRow>, bool)> {
        let mut walk = self.repo.revwalk()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        let mut pushed = false;
        if let Some(oid) = head.as_ref().and_then(|h| h.oid) {
            walk.push(oid)?;
            pushed = true;
        }
        for b in branches.iter().filter(|b| !b.is_remote) {
            if walk.push(b.oid).is_ok() {
                pushed = true;
            }
        }
        if !pushed {
            return Ok((Vec::new(), false));
        }
        let mut rows = Vec::with_capacity(limit.min(4096));
        let mut truncated = false;
        for oid in walk {
            let oid = oid?;
            if rows.len() >= limit {
                truncated = true;
                break;
            }
            let c = self.repo.find_commit(oid)?;
            let refs = labels
                .iter()
                .filter(|(o, _)| *o == oid)
                .map(|(_, l)| l.clone())
                .collect();
            let author = c.author();
            rows.push(CommitRow {
                oid,
                short: short_id(oid),
                parents: c.parent_ids().collect(),
                summary: c.summary().ok().flatten().unwrap_or("").to_owned(),
                body: c.body().ok().flatten().unwrap_or("").trim().to_owned(),
                author: author.name().unwrap_or("").to_owned(),
                email: author.email().unwrap_or("").to_owned(),
                time: author.when().seconds(),
                refs,
            });
        }
        Ok((rows, truncated))
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn status(&self) -> Result<(Vec<FileStatus>, Vec<FileStatus>, Vec<FileStatus>)> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .renames_index_to_workdir(true)
            .include_ignored(false);
        let statuses = self.repo.statuses(Some(&mut opts))?;
        let mut unstaged = Vec::new();
        let mut staged = Vec::new();
        let mut conflicted = Vec::new();
        for e in statuses.iter() {
            let s = e.status();
            let path = e.path().unwrap_or("").to_owned();
            if s.is_conflicted() {
                conflicted.push(FileStatus {
                    path,
                    old_path: None,
                    kind: FileKind::Conflicted,
                });
                continue;
            }
            if s.is_index_new()
                || s.is_index_modified()
                || s.is_index_deleted()
                || s.is_index_renamed()
                || s.is_index_typechange()
            {
                let kind = if s.is_index_new() {
                    FileKind::Added
                } else if s.is_index_deleted() {
                    FileKind::Deleted
                } else if s.is_index_renamed() {
                    FileKind::Renamed
                } else if s.is_index_typechange() {
                    FileKind::TypeChange
                } else {
                    FileKind::Modified
                };
                let (path, old_path) = e
                    .head_to_index()
                    .map(|d| {
                        let new = d
                            .new_file()
                            .path()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.clone());
                        let old = d
                            .old_file()
                            .path()
                            .map(|p| p.to_string_lossy().into_owned());
                        let old = old.filter(|o| kind == FileKind::Renamed && *o != new);
                        (new, old)
                    })
                    .unwrap_or((path.clone(), None));
                staged.push(FileStatus {
                    path,
                    old_path,
                    kind,
                });
            }
            if s.is_wt_new()
                || s.is_wt_modified()
                || s.is_wt_deleted()
                || s.is_wt_renamed()
                || s.is_wt_typechange()
            {
                let kind = if s.is_wt_new() {
                    FileKind::Untracked
                } else if s.is_wt_deleted() {
                    FileKind::Deleted
                } else if s.is_wt_renamed() {
                    FileKind::Renamed
                } else if s.is_wt_typechange() {
                    FileKind::TypeChange
                } else {
                    FileKind::Modified
                };
                let (path, old_path) = e
                    .index_to_workdir()
                    .map(|d| {
                        let new = d
                            .new_file()
                            .path()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.clone());
                        let old = d
                            .old_file()
                            .path()
                            .map(|p| p.to_string_lossy().into_owned());
                        let old = old.filter(|o| kind == FileKind::Renamed && *o != new);
                        (new, old)
                    })
                    .unwrap_or((path.clone(), None));
                unstaged.push(FileStatus {
                    path,
                    old_path,
                    kind,
                });
            }
        }
        let by_path = |a: &FileStatus, b: &FileStatus| a.path.cmp(&b.path);
        unstaged.sort_by(by_path);
        staged.sort_by(by_path);
        conflicted.sort_by(by_path);
        Ok((unstaged, staged, conflicted))
    }

    /// Files changed by a commit, against its first parent (or empty tree).
    pub fn commit_files(&self, oid: Oid) -> Result<Vec<FileStatus>> {
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;
        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let mut opts = git2::DiffOptions::new();
        let mut diff =
            self.repo
                .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;
        let mut find = git2::DiffFindOptions::new();
        find.renames(true);
        diff.find_similar(Some(&mut find))?;
        let mut out = Vec::new();
        for d in diff.deltas() {
            let new = d
                .new_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let old = d
                .old_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned());
            let kind = match d.status() {
                git2::Delta::Added => FileKind::Added,
                git2::Delta::Deleted => FileKind::Deleted,
                git2::Delta::Renamed => FileKind::Renamed,
                git2::Delta::Typechange => FileKind::TypeChange,
                _ => FileKind::Modified,
            };
            out.push(FileStatus {
                path: new.clone(),
                old_path: old.filter(|o| kind == FileKind::Renamed && *o != new),
                kind,
            });
        }
        Ok(out)
    }

    /// Diff text for one file.
    pub fn diff(&self, target: &DiffTarget, diff_opts: DiffOpts) -> Result<DiffText> {
        let path = target.path();
        if matches!(target, DiffTarget::WorkdirUnstaged(_)) && self.is_conflicted(path) {
            return Ok(conflict_view(target, &self.workdir.join(path)));
        }
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        diff_opts.apply(&mut opts);
        let diff = match target {
            DiffTarget::WorkdirUnstaged(_) => {
                self.repo.diff_index_to_workdir(None, Some(&mut opts))?
            }
            DiffTarget::Staged(_) => {
                let head_tree = self.repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                let index = self.index()?;
                self.repo
                    .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))?
            }
            DiffTarget::Commit(oid, _) => {
                let commit = self.repo.find_commit(*oid)?;
                let tree = commit.tree()?;
                let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
                self.repo
                    .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?
            }
        };
        let mut out = DiffText {
            target: target.clone(),
            binary: false,
            too_large: false,
            status: FileKind::Modified,
            hunks: Vec::new(),
        };
        let n = diff.deltas().len();
        for idx in 0..n {
            let delta = diff.get_delta(idx).expect("delta index in range");
            out.status = match delta.status() {
                git2::Delta::Added | git2::Delta::Untracked => FileKind::Added,
                git2::Delta::Deleted => FileKind::Deleted,
                git2::Delta::Renamed => FileKind::Renamed,
                git2::Delta::Typechange => FileKind::TypeChange,
                _ => FileKind::Modified,
            };
            let size = delta.new_file().size().max(delta.old_file().size());
            if size > MAX_DIFF_BYTES {
                out.too_large = true;
                continue;
            }
            if delta.new_file().is_binary() || delta.old_file().is_binary() {
                out.binary = true;
                continue;
            }
            let Some(patch) = git2::Patch::from_diff(&diff, idx)? else {
                continue;
            };
            for h in 0..patch.num_hunks() {
                let (hunk, count) = patch.hunk(h)?;
                let mut lines = Vec::with_capacity(count);
                for l in 0..count {
                    let line = patch.line_in_hunk(h, l)?;
                    let origin = line.origin();
                    if matches!(origin, '<' | '>' | '=') {
                        // "\ No newline at end of file" applies to the line before.
                        if let Some(prev) = lines.last_mut() {
                            let prev: &mut DiffLine = prev;
                            prev.no_newline = true;
                        }
                        continue;
                    }
                    if !matches!(origin, '+' | '-' | ' ') {
                        continue;
                    }
                    let mut text = String::from_utf8_lossy(line.content()).into_owned();
                    while text.ends_with('\n') || text.ends_with('\r') {
                        text.pop();
                    }
                    lines.push(DiffLine {
                        origin,
                        old_no: line.old_lineno(),
                        new_no: line.new_lineno(),
                        text,
                        no_newline: false,
                    });
                }
                let header = String::from_utf8_lossy(hunk.header()).trim_end().to_owned();
                out.hunks.push(Hunk { header, lines });
            }
        }
        Ok(out)
    }
    fn is_conflicted(&self, path: &str) -> bool {
        self.index()
            .ok()
            .is_some_and(|i| i.has_conflicts() && i.conflict_get(Path::new(path)).is_ok())
    }

    // ---- writes (Phase 4) ----

    /// The repository index, re-read from disk when another process (the
    /// git CLI, an editor, an agent in the next pane) wrote it since libgit2
    /// last loaded it. libgit2 caches the index per repository and most
    /// operations do not refresh it on their own.
    pub(crate) fn index(&self) -> std::result::Result<git2::Index, git2::Error> {
        let mut index = self.repo.index()?;
        index.read(false)?;
        Ok(index)
    }

    fn index_write(&self, index: &mut git2::Index) -> Result<()> {
        index.write()?;
        Ok(())
    }

    /// Stage paths: add or update existing files (and directories), remove
    /// entries whose file is gone from the working tree.
    pub fn stage(&self, paths: &[String]) -> Result<()> {
        let mut index = self.index()?;
        for p in paths {
            let full = self.workdir.join(p);
            if full.exists() {
                index.add_all([p.as_str()], git2::IndexAddOption::DEFAULT, None)?;
            } else {
                // Deleted file or directory.
                if index.get_path(Path::new(p), 0).is_some() {
                    index.remove_path(Path::new(p))?;
                } else {
                    index.remove_dir(Path::new(p), 0)?;
                }
            }
        }
        self.index_write(&mut index)
    }

    pub fn stage_all(&self) -> Result<()> {
        let mut index = self.index()?;
        index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
        index.update_all(["*"], None)?;
        self.index_write(&mut index)
    }

    /// Reset index entries to HEAD (or drop them when there is no HEAD yet).
    pub fn unstage(&self, paths: &[String]) -> Result<()> {
        match self
            .repo
            .head()
            .ok()
            .and_then(|h| h.peel(git2::ObjectType::Commit).ok())
        {
            Some(head) => {
                self.repo
                    .reset_default(Some(&head), paths.iter().map(|s| s.as_str()))?;
            }
            None => {
                let mut index = self.index()?;
                for p in paths {
                    let _ = index.remove_path(Path::new(p));
                    let _ = index.remove_dir(Path::new(p), 0);
                }
                self.index_write(&mut index)?;
            }
        }
        Ok(())
    }

    pub fn unstage_all(&self) -> Result<()> {
        let (_, staged, _) = self.status()?;
        let mut paths: Vec<String> = staged.iter().map(|f| f.path.clone()).collect();
        paths.extend(staged.iter().filter_map(|f| f.old_path.clone()));
        if paths.is_empty() {
            return Ok(());
        }
        self.unstage(&paths)
    }

    /// Throw away working tree changes for paths: tracked files are restored
    /// from the index, untracked files and directories are deleted.
    pub fn discard(&self, paths: &[String]) -> Result<()> {
        let index = self.index()?;
        let mut tracked = Vec::new();
        for p in paths {
            if index.get_path(Path::new(p), 0).is_some() {
                tracked.push(p.clone());
            } else {
                let full = self.workdir.join(p);
                if full.is_dir() {
                    std::fs::remove_dir_all(&full)
                        .map_err(|e| git2::Error::from_str(&e.to_string()))?;
                } else if full.exists() {
                    std::fs::remove_file(&full)
                        .map_err(|e| git2::Error::from_str(&e.to_string()))?;
                }
            }
        }
        if !tracked.is_empty() {
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.force();
            for p in &tracked {
                cb.path(p);
            }
            self.repo.checkout_index(None, Some(&mut cb))?;
        }
        Ok(())
    }

    /// Stage only hunk `hunk_index` of the unstaged diff for `path`. The
    /// hunk index refers to a diff produced with the same `diff_opts`.
    pub fn stage_hunk(&self, path: &str, hunk_index: usize, diff_opts: DiffOpts) -> Result<()> {
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path)
            .include_untracked(true)
            .show_untracked_content(true);
        diff_opts.apply(&mut opts);
        let diff = self.repo.diff_index_to_workdir(None, Some(&mut opts))?;
        self.apply_hunk_to_index(&diff, hunk_index)
    }

    /// Remove only hunk `hunk_index` of the staged diff for `path` from the index.
    pub fn unstage_hunk(&self, path: &str, hunk_index: usize, diff_opts: DiffOpts) -> Result<()> {
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path);
        diff_opts.apply(&mut opts);
        let head_tree = self.repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        let index = self.index()?;
        let diff =
            self.repo
                .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))?;
        let mut text = Vec::new();
        for i in 0..diff.deltas().len() {
            if let Some(mut patch) = git2::Patch::from_diff(&diff, i)? {
                text.extend_from_slice(&patch.to_buf()?);
            }
        }
        let reversed = reverse_patch(&text);
        let rdiff = git2::Diff::from_buffer(&reversed)?;
        self.apply_hunk_to_index(&rdiff, hunk_index)
    }

    fn apply_hunk_to_index(&self, diff: &git2::Diff<'_>, hunk_index: usize) -> Result<()> {
        let mut seen = 0usize;
        let mut opts = git2::ApplyOptions::new();
        opts.hunk_callback(move |_hunk| {
            let keep = seen == hunk_index;
            seen += 1;
            keep
        });
        self.repo
            .apply(diff, git2::ApplyLocation::Index, Some(&mut opts))?;
        Ok(())
    }

    /// True when `commit.gpgsign` is set, in which case commits go through
    /// the git CLI so the user's signing setup is used.
    pub fn gpgsign(&self) -> bool {
        self.repo
            .config()
            .ok()
            .and_then(|c| c.get_bool("commit.gpgsign").ok())
            .unwrap_or(false)
    }

    pub fn commit(&self, message: &str, amend: bool) -> Result<Oid> {
        if self.gpgsign() {
            return self.commit_via_cli(message, amend);
        }
        let sig = self.repo.signature()?;
        let mut index = self.index()?;
        let tree_id = index.write_tree()?;
        let tree = self.repo.find_tree(tree_id)?;
        if amend {
            let head = self.repo.head()?.peel_to_commit()?;
            return Ok(head.amend(
                Some("HEAD"),
                None,
                Some(&sig),
                None,
                Some(message),
                Some(&tree),
            )?);
        }
        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        Ok(self
            .repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?)
    }

    fn commit_via_cli(&self, message: &str, amend: bool) -> Result<Oid> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("commit").arg("-m").arg(message);
        if amend {
            cmd.arg("--amend");
        }
        let out = cmd
            .current_dir(&self.workdir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| git2::Error::from_str(&format!("running git: {e}")))?;
        if !out.status.success() {
            return Err(git2::Error::from_str(String::from_utf8_lossy(&out.stderr).trim()).into());
        }
        Ok(self.repo.head()?.peel_to_commit()?.id())
    }

    /// Message of the HEAD commit, for amend.
    pub fn head_message(&self) -> Option<String> {
        self.repo
            .head()
            .ok()?
            .peel_to_commit()
            .ok()?
            .message()
            .ok()
            .map(|m| m.to_owned())
    }

    /// Check out a local branch, or a remote branch by creating a local
    /// tracking branch of the same short name first.
    /// When `force` is true, local changes that block checkout are discarded.
    pub fn checkout(&self, name: &str, force: bool) -> Result<String> {
        let local = if self.repo.find_branch(name, git2::BranchType::Local).is_ok() {
            name.to_owned()
        } else {
            let remote = self.repo.find_branch(name, git2::BranchType::Remote)?;
            let short = name
                .split_once('/')
                .map(|(_, b)| b)
                .unwrap_or(name)
                .to_owned();
            let target = remote.get().peel_to_commit()?;
            let mut b = match self.repo.find_branch(&short, git2::BranchType::Local) {
                Ok(b) => b,
                Err(_) => self.repo.branch(&short, &target, false)?,
            };
            let _ = b.set_upstream(Some(name));
            short
        };
        let refname = format!("refs/heads/{local}");
        let obj = self.repo.revparse_single(&refname)?;
        let mut cb = git2::build::CheckoutBuilder::new();
        if force {
            cb.force();
        } else {
            cb.safe();
        }
        self.repo.checkout_tree(&obj, Some(&mut cb))?;
        self.repo.set_head(&refname)?;
        Ok(local)
    }

    pub fn create_branch(&self, name: &str, from: Oid, checkout: bool) -> Result<()> {
        let commit = self.repo.find_commit(from)?;
        self.repo.branch(name, &commit, false)?;
        if checkout {
            self.checkout(name, false)?;
        }
        Ok(())
    }

    /// Stash the working tree, then check out `branch`.
    pub fn stash_and_checkout(&mut self, message: &str, branch: &str) -> Result<String> {
        self.stash_push(message)?;
        self.checkout(branch, false)
    }

    pub fn delete_branch(&self, name: &str) -> Result<()> {
        let head = self.head_info().and_then(|h| h.branch_name);
        if head.as_deref() == Some(name) {
            return Err(git2::Error::from_str("cannot delete the checked out branch").into());
        }
        let mut b = self.repo.find_branch(name, git2::BranchType::Local)?;
        b.delete()?;
        Ok(())
    }

    pub fn stash_push(&mut self, message: &str) -> Result<()> {
        self.stash_push_opts(message, false, true)
    }

    /// Stash with options: `keep_index` leaves staged changes in the index,
    /// `include_untracked` stashes untracked files too.
    pub fn stash_push_opts(
        &mut self,
        message: &str,
        keep_index: bool,
        include_untracked: bool,
    ) -> Result<()> {
        let sig = self.repo.signature()?;
        let msg = if message.trim().is_empty() {
            "gitgui stash"
        } else {
            message
        };
        let mut flags = git2::StashFlags::DEFAULT;
        if keep_index {
            flags |= git2::StashFlags::KEEP_INDEX;
        }
        if include_untracked {
            flags |= git2::StashFlags::INCLUDE_UNTRACKED;
        }
        self.repo.stash_save(&sig, msg, Some(flags))?;
        Ok(())
    }

    /// Apply a stash without dropping it.
    pub fn stash_apply(&mut self, index: usize) -> Result<()> {
        self.repo.stash_apply(index, None)?;
        Ok(())
    }

    pub fn stash_pop(&mut self, index: usize) -> Result<()> {
        self.repo.stash_pop(index, None)?;
        Ok(())
    }

    pub fn stash_drop(&mut self, index: usize) -> Result<()> {
        self.repo.stash_drop(index)?;
        Ok(())
    }
}

/// A conflicted file cannot be diffed against the index, so show the working
/// tree file itself: lines of the "ours" block as removals, lines of the
/// "theirs" block as additions, everything else as context. Markers stay
/// visible so the user knows what they are looking at.
pub fn conflict_view(target: &DiffTarget, file: &Path) -> DiffText {
    let text = std::fs::read(file).unwrap_or_default();
    let mut out = DiffText {
        target: target.clone(),
        binary: false,
        too_large: false,
        status: FileKind::Conflicted,
        hunks: Vec::new(),
    };
    if text.len() as u64 > MAX_DIFF_BYTES {
        out.too_large = true;
        return out;
    }
    if text.contains(&0u8) {
        out.binary = true;
        return out;
    }
    let text = String::from_utf8_lossy(&text);
    let mut lines = Vec::new();
    let mut side = ' ';
    let mut conflicts = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let no = Some(i as u32 + 1);
        let origin = if raw.starts_with("<<<<<<< ") {
            side = '-';
            conflicts += 1;
            ' '
        } else if raw.starts_with("=======") && side == '-' {
            side = '+';
            ' '
        } else if raw.starts_with(">>>>>>> ") && side == '+' {
            side = ' ';
            ' '
        } else if raw.starts_with("||||||| ") && side == '-' {
            // diff3 style: the base block is shown as context.
            side = ' ';
            ' '
        } else if raw.starts_with("=======") && side == ' ' && conflicts > 0 {
            side = '+';
            ' '
        } else {
            side
        };
        lines.push(DiffLine {
            origin,
            old_no: if origin == '+' { None } else { no },
            new_no: if origin == '-' { None } else { no },
            text: raw.to_owned(),
            no_newline: false,
        });
    }
    out.hunks.push(Hunk {
        header: format!(
            "@@ {conflicts} conflict{}: <<<<<<< ours shown as removed, >>>>>>> theirs as added @@",
            if conflicts == 1 { "" } else { "s" }
        ),
        lines,
    });
    out
}

/// Reverse a unified diff: swap the file headers, the hunk ranges and the
/// +/- line prefixes. Lines starting with `\` (no newline markers) and
/// `diff --git`/`index` headers are kept as they are.
pub fn reverse_patch(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for line in text.split_inclusive(|&b| b == b'\n') {
        let (body, nl): (&[u8], &[u8]) = match line.strip_suffix(b"\n") {
            Some(b) => (b, b"\n"),
            None => (line, b""),
        };
        if body.starts_with(b"--- ") || body.starts_with(b"+++ ") {
            // File headers stay as they are: libgit2 checks them against the
            // "diff --git a/x b/x" line, and both sides name the same file.
            out.extend_from_slice(body);
        } else if body.starts_with(b"@@ ") {
            out.extend_from_slice(&reverse_hunk_header(body));
        } else if body.starts_with(b"+") {
            out.push(b'-');
            out.extend_from_slice(&body[1..]);
        } else if body.starts_with(b"-") {
            out.push(b'+');
            out.extend_from_slice(&body[1..]);
        } else {
            out.extend_from_slice(body);
        }
        out.extend_from_slice(nl);
    }
    out
}

fn reverse_hunk_header(body: &[u8]) -> Vec<u8> {
    // @@ -a,b +c,d @@ rest
    let s = String::from_utf8_lossy(body);
    let mut parts = s.splitn(4, ' ');
    let (Some(_), Some(old), Some(new), rest) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return body.to_vec();
    };
    let old = old.strip_prefix('-').unwrap_or(old);
    let new = new.strip_prefix('+').unwrap_or(new);
    let mut out = format!("@@ -{new} +{old}");
    match rest {
        Some(r) => {
            out.push(' ');
            out.push_str(r);
        }
        None => out.push_str(" @@"),
    }
    out.into_bytes()
}

pub fn short_id(oid: Oid) -> String {
    let s = oid.to_string();
    s[..7.min(s.len())].to_owned()
}

#[cfg(test)]
pub mod testutil {
    use std::path::{Path, PathBuf};

    /// A throwaway repository with a configured identity.
    pub struct TempRepo {
        pub dir: PathBuf,
        pub repo: git2::Repository,
    }

    impl TempRepo {
        pub fn new() -> Self {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("gitgui-test-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let repo = git2::Repository::init(&dir).unwrap();
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test User").unwrap();
            cfg.set_str("user.email", "test@example.com").unwrap();
            drop(cfg);
            TempRepo { dir, repo }
        }

        pub fn write(&self, rel: &str, content: &str) {
            let p = self.dir.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, content).unwrap();
        }

        /// The index, re-read when a `Repo` under test wrote it meanwhile.
        fn index(&self) -> git2::Index {
            let mut idx = self.repo.index().unwrap();
            idx.read(false).unwrap();
            idx
        }

        pub fn add(&self, rel: &str) {
            let mut idx = self.index();
            idx.add_path(Path::new(rel)).unwrap();
            idx.write().unwrap();
        }

        pub fn commit(&self, message: &str) -> git2::Oid {
            let mut idx = self.index();
            let tree_id = idx.write_tree().unwrap();
            let tree = self.repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("Test User", "test@example.com").unwrap();
            let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
            let parents: Vec<&git2::Commit> = parent.iter().collect();
            self.repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
                .unwrap()
        }

        pub fn commit_file(&self, rel: &str, content: &str, message: &str) -> git2::Oid {
            self.write(rel, content);
            self.add(rel);
            self.commit(message)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::TempRepo;
    use super::*;

    #[test]
    fn not_a_repo() {
        let dir = std::env::temp_dir().join(format!("gitgui-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = Repo::open(&dir).err().expect("should fail");
        assert!(matches!(err, GitError::NotARepository(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_creates_repo() {
        let dir = std::env::temp_dir().join(format!("gitgui-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Repo::init(&dir).expect("init");
        assert!(dir.join(".git").exists());
        assert!(Repo::open(&dir).is_ok());
        let _ = r;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_repo_snapshot() {
        let t = TempRepo::new();
        let mut r = Repo::open(&t.dir).unwrap();
        let s = r.snapshot(100).unwrap();
        assert!(s.commits.is_empty());
        assert_eq!(s.head.as_ref().unwrap().oid, None);
        assert!(s.head.as_ref().unwrap().branch_name.is_some());
        assert_eq!(s.user_name, "Test User");
    }

    #[test]
    fn worktree_fingerprint_changes_on_edit() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "x\n", "init");
        let repo = Repo::open(&t.dir).unwrap();
        let before = repo.worktree_fingerprint().unwrap();
        t.write("a.txt", "changed\n");
        let after = repo.worktree_fingerprint().unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn status_categories() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.commit_file("del.txt", "x\n", "add del");
        t.commit_file(
            "ren.txt",
            "same content for rename detection\nline2\nline3\n",
            "add ren",
        );
        // staged modification of a.txt
        t.write("a.txt", "one\ntwo\n");
        t.add("a.txt");
        // unstaged modification on top
        t.write("a.txt", "one\ntwo\nthree\n");
        // untracked
        t.write("new.txt", "hi\n");
        // staged new file
        t.write("added.txt", "added\n");
        t.add("added.txt");
        // deleted in worktree
        std::fs::remove_file(t.dir.join("del.txt")).unwrap();
        // staged rename
        std::fs::rename(t.dir.join("ren.txt"), t.dir.join("ren2.txt")).unwrap();
        let mut idx = t.repo.index().unwrap();
        idx.remove_path(Path::new("ren.txt")).unwrap();
        idx.add_path(Path::new("ren2.txt")).unwrap();
        idx.write().unwrap();

        let mut r = Repo::open(&t.dir).unwrap();
        let s = r.snapshot(100).unwrap();
        let find = |v: &[FileStatus], p: &str| v.iter().find(|f| f.path == p).cloned();
        assert_eq!(find(&s.staged, "a.txt").unwrap().kind, FileKind::Modified);
        assert_eq!(find(&s.unstaged, "a.txt").unwrap().kind, FileKind::Modified);
        assert_eq!(
            find(&s.unstaged, "new.txt").unwrap().kind,
            FileKind::Untracked
        );
        assert_eq!(find(&s.staged, "added.txt").unwrap().kind, FileKind::Added);
        assert_eq!(
            find(&s.unstaged, "del.txt").unwrap().kind,
            FileKind::Deleted
        );
        let ren = find(&s.staged, "ren2.txt").unwrap();
        assert_eq!(ren.kind, FileKind::Renamed);
        assert_eq!(ren.old_path.as_deref(), Some("ren.txt"));
        assert!(s.is_dirty());
        assert_eq!(s.commits.len(), 3);
        assert_eq!(s.commits[0].summary, "add ren");
        assert!(s.branches.iter().any(|b| b.is_head));
    }

    #[test]
    fn diffs_for_each_target() {
        let t = TempRepo::new();
        let c1 = t.commit_file("f.txt", "a\nb\nc\n", "first");
        t.write("f.txt", "a\nB\nc\n");
        t.add("f.txt");
        t.write("f.txt", "a\nB\nc\nd\n");
        let r = Repo::open(&t.dir).unwrap();

        let staged = r.diff(&DiffTarget::Staged("f.txt".into()), DiffOpts::default()).unwrap();
        assert_eq!(staged.hunks.len(), 1);
        let lines: Vec<(char, &str)> = staged.hunks[0]
            .lines
            .iter()
            .map(|l| (l.origin, l.text.as_str()))
            .collect();
        assert_eq!(lines, vec![(' ', "a"), ('-', "b"), ('+', "B"), (' ', "c")]);
        assert_eq!(staged.hunks[0].lines[1].old_no, Some(2));
        assert_eq!(staged.hunks[0].lines[2].new_no, Some(2));
        assert!(staged.hunks[0].header.starts_with("@@ -1,3 +1,3 @@"));

        let unstaged = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), DiffOpts::default())
            .unwrap();
        let lines: Vec<(char, &str)> = unstaged.hunks[0]
            .lines
            .iter()
            .map(|l| (l.origin, l.text.as_str()))
            .collect();
        assert_eq!(lines, vec![(' ', "a"), (' ', "B"), (' ', "c"), ('+', "d")]);

        let commit = r.diff(&DiffTarget::Commit(c1, "f.txt".into()), DiffOpts::default()).unwrap();
        assert_eq!(
            commit.hunks[0]
                .lines
                .iter()
                .filter(|l| l.origin == '+')
                .count(),
            3
        );
        let files = r.commit_files(c1).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].kind, FileKind::Added);

        // untracked file diff shows its content as additions
        t.write("u.txt", "x\ny\n");
        let u = r
            .diff(&DiffTarget::WorkdirUnstaged("u.txt".into()), DiffOpts::default())
            .unwrap();
        assert_eq!(
            u.hunks[0].lines.iter().filter(|l| l.origin == '+').count(),
            2
        );
    }

    #[test]
    fn branches_tags_and_refs() {
        let t = TempRepo::new();
        let c1 = t.commit_file("a", "1", "one");
        let c2 = t.commit_file("a", "2", "two");
        t.repo
            .tag_lightweight("v1", &t.repo.find_object(c1, None).unwrap(), false)
            .unwrap();
        t.repo
            .branch("feature", &t.repo.find_commit(c2).unwrap(), false)
            .unwrap();
        let mut r = Repo::open(&t.dir).unwrap();
        let s = r.snapshot(100).unwrap();
        let names: Vec<&str> = s.branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature"));
        assert_eq!(
            s.tags,
            vec![Tag {
                name: "v1".into(),
                oid: c1
            }]
        );
        let top = &s.commits[0];
        assert!(top
            .refs
            .iter()
            .any(|l| l.name == "feature" && l.kind == RefKind::LocalBranch));
        assert!(s.commits[1]
            .refs
            .iter()
            .any(|l| l.name == "v1" && l.kind == RefKind::Tag));
        assert_eq!(top.short.len(), 7);
    }

    #[test]
    fn log_is_capped() {
        let t = TempRepo::new();
        for i in 0..5 {
            t.commit_file("a", &i.to_string(), &format!("c{i}"));
        }
        let mut r = Repo::open(&t.dir).unwrap();
        let s = r.snapshot(3).unwrap();
        assert_eq!(s.commits.len(), 3);
        assert!(s.truncated);
        assert_eq!(s.commits[0].summary, "c4");
    }
    #[test]
    fn stage_unstage_files_and_all() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "a\n", "init");
        t.commit_file("gone.txt", "g\n", "add gone");
        let r = Repo::open(&t.dir).unwrap();
        t.write("a.txt", "a2\n");
        t.write("new.txt", "n\n");
        t.write("dir/sub.txt", "s\n");
        std::fs::remove_file(t.dir.join("gone.txt")).unwrap();

        r.stage(&["a.txt".into(), "dir".into(), "gone.txt".into()])
            .unwrap();
        let (unstaged, staged, _) = r.status().unwrap();
        let names = |v: &[FileStatus]| v.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
        assert_eq!(names(&staged), vec!["a.txt", "dir/sub.txt", "gone.txt"]);
        assert_eq!(names(&unstaged), vec!["new.txt"]);
        assert_eq!(
            staged.iter().find(|f| f.path == "gone.txt").unwrap().kind,
            FileKind::Deleted
        );

        r.unstage(&["a.txt".into(), "dir/sub.txt".into()]).unwrap();
        let (unstaged, staged, _) = r.status().unwrap();
        assert_eq!(names(&staged), vec!["gone.txt"]);
        assert_eq!(names(&unstaged), vec!["a.txt", "dir/sub.txt", "new.txt"]);

        r.stage_all().unwrap();
        let (unstaged, staged, _) = r.status().unwrap();
        assert!(unstaged.is_empty());
        assert_eq!(staged.len(), 4);

        r.unstage_all().unwrap();
        let (unstaged, staged, _) = r.status().unwrap();
        assert!(staged.is_empty());
        assert_eq!(unstaged.len(), 4);
    }

    #[test]
    fn discard_restores_tracked_and_removes_untracked() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "orig\n", "init");
        let r = Repo::open(&t.dir).unwrap();
        t.write("a.txt", "changed\n");
        t.write("junk.txt", "x\n");
        t.write("junkdir/f.txt", "x\n");
        r.discard(&["a.txt".into(), "junk.txt".into(), "junkdir".into()])
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(t.dir.join("a.txt")).unwrap(),
            "orig\n"
        );
        assert!(!t.dir.join("junk.txt").exists());
        assert!(!t.dir.join("junkdir").exists());
        let (unstaged, staged, _) = r.status().unwrap();
        assert!(unstaged.is_empty() && staged.is_empty());
    }

    fn two_hunk_file() -> String {
        let mut s = String::new();
        for i in 1..=30 {
            s.push_str(&format!("line {i}\n"));
        }
        s
    }

    #[test]
    fn stage_and_unstage_single_hunk_round_trip() {
        let t = TempRepo::new();
        let base = two_hunk_file();
        t.commit_file("f.txt", &base, "init");
        let r = Repo::open(&t.dir).unwrap();
        // Change line 2 and line 28: two separate hunks.
        let modified = base
            .replace("line 2\n", "LINE 2\n")
            .replace("line 28\n", "LINE 28\n");
        t.write("f.txt", &modified);
        let d = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), DiffOpts::default())
            .unwrap();
        assert_eq!(d.hunks.len(), 2);

        r.stage_hunk("f.txt", 1, DiffOpts::default()).unwrap();
        let staged = r.diff(&DiffTarget::Staged("f.txt".into()), DiffOpts::default()).unwrap();
        assert_eq!(staged.hunks.len(), 1);
        assert!(staged.hunks[0]
            .lines
            .iter()
            .any(|l| l.origin == '+' && l.text == "LINE 28"));
        let unstaged = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), DiffOpts::default())
            .unwrap();
        assert_eq!(unstaged.hunks.len(), 1);
        assert!(unstaged.hunks[0]
            .lines
            .iter()
            .any(|l| l.origin == '+' && l.text == "LINE 2"));

        // Stage the other hunk too, then unstage only the first one.
        r.stage_hunk("f.txt", 0, DiffOpts::default()).unwrap();
        assert_eq!(
            r.diff(&DiffTarget::Staged("f.txt".into()), DiffOpts::default())
                .unwrap()
                .hunks
                .len(),
            2
        );
        assert!(r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), DiffOpts::default())
            .unwrap()
            .hunks
            .is_empty());
        r.unstage_hunk("f.txt", 0, DiffOpts::default()).unwrap();
        let staged = r.diff(&DiffTarget::Staged("f.txt".into()), DiffOpts::default()).unwrap();
        assert_eq!(staged.hunks.len(), 1);
        assert!(staged.hunks[0].lines.iter().any(|l| l.text == "LINE 28"));
        let unstaged = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), DiffOpts::default())
            .unwrap();
        assert_eq!(unstaged.hunks.len(), 1);
        assert!(unstaged.hunks[0].lines.iter().any(|l| l.text == "LINE 2"));
        // Unstage the remaining hunk: index equals HEAD again, worktree unchanged.
        r.unstage_hunk("f.txt", 0, DiffOpts::default()).unwrap();
        assert!(r
            .diff(&DiffTarget::Staged("f.txt".into()), DiffOpts::default())
            .unwrap()
            .hunks
            .is_empty());
        assert_eq!(
            std::fs::read_to_string(t.dir.join("f.txt")).unwrap(),
            modified
        );
    }

    #[test]
    fn conflict_view_marks_sides() {
        let dir = std::env::temp_dir().join(format!("gitgui-conflict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.txt");
        std::fs::write(
            &f,
            "one\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> side\nthree\n",
        )
        .unwrap();
        let d = conflict_view(&DiffTarget::WorkdirUnstaged("c.txt".into()), &f);
        assert_eq!(d.status, FileKind::Conflicted);
        let origins: String = d.hunks[0].lines.iter().map(|l| l.origin).collect();
        assert_eq!(origins, "  - +  ");
        assert!(d.hunks[0].header.contains("1 conflict:"));
        assert_eq!(d.hunks[0].lines[2].text, "ours");
        assert_eq!(d.hunks[0].lines[4].text, "theirs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reverse_patch_text() {
        let patch = b"diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@ ctx\n a\n-b\n+B\n+c\n d\n\\ No newline at end of file\n";
        let rev = reverse_patch(patch);
        let text = String::from_utf8(rev).unwrap();
        assert_eq!(
            text,
            "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1,4 +1,3 @@ ctx\n a\n+b\n-B\n-c\n d\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn commit_and_amend() {
        let t = TempRepo::new();
        let r = Repo::open(&t.dir).unwrap();
        t.write("a.txt", "1\n");
        r.stage(&["a.txt".into()]).unwrap();
        let c1 = r.commit("first", false).unwrap();
        assert_eq!(t.repo.find_commit(c1).unwrap().message().unwrap(), "first");
        assert_eq!(t.repo.find_commit(c1).unwrap().parent_count(), 0);
        t.write("b.txt", "2\n");
        r.stage(&["b.txt".into()]).unwrap();
        let c2 = r.commit("second", false).unwrap();
        assert_eq!(t.repo.find_commit(c2).unwrap().parent_id(0).unwrap(), c1);
        assert_eq!(r.head_message().as_deref(), Some("second"));
        t.write("c.txt", "3\n");
        r.stage(&["c.txt".into()]).unwrap();
        let c3 = r.commit("second, amended", true).unwrap();
        let head = t.repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.id(), c3);
        assert_eq!(head.parent_id(0).unwrap(), c1);
        assert!(head.tree().unwrap().get_name("c.txt").is_some());
        let (unstaged, staged, _) = r.status().unwrap();
        assert!(unstaged.is_empty() && staged.is_empty());
    }

    #[test]
    fn branches_checkout_create_delete() {
        let t = TempRepo::new();
        let c1 = t.commit_file("a.txt", "1\n", "one");
        t.commit_file("a.txt", "2\n", "two");
        let r = Repo::open(&t.dir).unwrap();
        r.create_branch("old", c1, false).unwrap();
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "2\n");
        assert_eq!(r.checkout("old", false).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "1\n");
        assert_eq!(r.head_info().unwrap().branch_name.as_deref(), Some("old"));
        assert!(
            r.delete_branch("old").is_err(),
            "cannot delete current branch"
        );
        let main = t
            .repo
            .branches(Some(git2::BranchType::Local))
            .unwrap()
            .flatten()
            .map(|(b, _)| b.name().unwrap().unwrap().to_owned())
            .find(|n| n != "old")
            .unwrap();
        r.checkout(&main, false).unwrap();
        r.delete_branch("old").unwrap();
        assert!(t.repo.find_branch("old", git2::BranchType::Local).is_err());
        // Remote branch checkout creates a local tracking branch.
        t.repo
            .remote("origin", "https://example.invalid/repo.git")
            .unwrap();
        let c = t.repo.head().unwrap().peel_to_commit().unwrap();
        t.repo
            .reference("refs/remotes/origin/feature", c.id(), false, "")
            .unwrap();
        assert_eq!(r.checkout("origin/feature", false).unwrap(), "feature");
        let b = t
            .repo
            .find_branch("feature", git2::BranchType::Local)
            .unwrap();
        assert!(b.upstream().is_ok());
    }

    #[test]
    fn stash_push_pop_drop() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "1\n", "one");
        let mut r = Repo::open(&t.dir).unwrap();
        t.write("a.txt", "dirty\n");
        t.write("u.txt", "untracked\n");
        r.stash_push("wip").unwrap();
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "1\n");
        assert!(!t.dir.join("u.txt").exists());
        let s = r.snapshot(10).unwrap();
        assert_eq!(s.stashes.len(), 1);
        assert!(s.stashes[0].message.contains("wip"));
        r.stash_pop(0).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.dir.join("a.txt")).unwrap(),
            "dirty\n"
        );
        assert!(t.dir.join("u.txt").exists());
        r.stash_push("again").unwrap();
        r.stash_drop(0).unwrap();
        assert!(r.snapshot(10).unwrap().stashes.is_empty());
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "1\n");
    }
}
