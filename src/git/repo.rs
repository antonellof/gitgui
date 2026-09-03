//! Repository access through git2. Everything here runs on the worker thread.

use std::fmt;
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
    pub hunks: Vec<Hunk>,
}

/// Files above this size are not diffed.
pub const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct RepoSnapshot {
    pub path: PathBuf,
    pub head: Option<HeadInfo>,
    pub branches: Vec<Branch>,
    pub tags: Vec<Tag>,
    pub stashes: Vec<Stash>,
    pub remotes: Vec<String>,
    pub commits: Vec<CommitRow>,
    pub graph: GraphLayout,
    /// True when more commits exist beyond the cap.
    pub truncated: bool,
    pub unstaged: Vec<FileStatus>,
    pub staged: Vec<FileStatus>,
    pub conflicted: Vec<FileStatus>,
    pub user_name: String,
    pub user_email: String,
}

impl RepoSnapshot {
    pub fn is_dirty(&self) -> bool {
        !self.unstaged.is_empty() || !self.staged.is_empty() || !self.conflicted.is_empty()
    }
}

pub struct Repo {
    repo: Repository,
    workdir: PathBuf,
}

impl Repo {
    /// Discover the repository containing `path`.
    pub fn open(path: &Path) -> Result<Repo> {
        let repo = Repository::discover(path).map_err(|_| GitError::NotARepository(path.to_path_buf()))?;
        let workdir = repo.workdir().map(|p| p.to_path_buf()).unwrap_or_else(|| repo.path().to_path_buf());
        Ok(Repo { repo, workdir })
    }

    /// Paths whose mtime signals repository or working tree changes.
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        let g = self.repo.path().to_path_buf();
        vec![g.join("HEAD"), g.join("index"), g.join("refs"), g.join("packed-refs"), g.join("refs/heads"), self.workdir.clone()]
    }

    pub fn snapshot(&mut self, commit_limit: usize) -> Result<Arc<RepoSnapshot>> {
        let head = self.head_info();
        let (branches, mut labels) = self.branches(&head)?;
        let tags = self.tags(&mut labels)?;
        if let Some(h) = &head {
            if let (Some(oid), true) = (h.oid, h.detached) {
                labels.push((oid, RefLabel { name: "HEAD".into(), kind: RefKind::Head }));
            }
        }
        let stashes = self.stashes()?;
        let remotes = self.repo.remotes().map(|r| r.iter().flatten().flatten().map(|s| s.to_owned()).collect()).unwrap_or_default();
        let (commits, truncated) = self.log(&branches, &head, &labels, commit_limit)?;
        let graph = graph::layout(&commits);
        let (unstaged, staged, conflicted) = self.status()?;
        let cfg = self.repo.config().ok();
        let user_name = cfg.as_ref().and_then(|c| c.get_string("user.name").ok()).unwrap_or_default();
        let user_email = cfg.as_ref().and_then(|c| c.get_string("user.email").ok()).unwrap_or_default();
        Ok(Arc::new(RepoSnapshot {
            path: self.workdir.clone(),
            head,
            branches,
            tags,
            stashes,
            remotes,
            commits,
            graph,
            truncated,
            unstaged,
            staged,
            conflicted,
            user_name,
            user_email,
        }))
    }

    fn head_info(&self) -> Option<HeadInfo> {
        let head = self.repo.head().ok();
        let detached = self.repo.head_detached().unwrap_or(false);
        match head {
            Some(r) => Some(HeadInfo {
                branch_name: if detached { None } else { r.shorthand().ok().map(|s| s.to_owned()) },
                oid: r.target(),
                detached,
            }),
            None => {
                // Unborn branch: HEAD points to a ref that does not exist yet.
                let name = std::fs::read_to_string(self.repo.path().join("HEAD"))
                    .ok()
                    .and_then(|s| s.trim().strip_prefix("ref: refs/heads/").map(|b| b.to_owned()));
                Some(HeadInfo { branch_name: name, oid: None, detached: false })
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
            let Some(name) = branch.name()?.map(|s| s.to_owned()) else { continue };
            let Some(oid) = branch.get().target() else { continue };
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
                    kind: if is_remote { RefKind::RemoteBranch } else { RefKind::LocalBranch },
                },
            ));
            out.push(Branch { name, oid, is_remote, upstream, ahead, behind, is_head });
        }
        out.sort_by(|a, b| a.is_remote.cmp(&b.is_remote).then_with(|| a.name.cmp(&b.name)));
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
            labels.push((target, RefLabel { name: short.clone(), kind: RefKind::Tag }));
            out.push(Tag { name: short, oid: target });
            true
        })?;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn stashes(&mut self) -> Result<Vec<Stash>> {
        let mut out = Vec::new();
        self.repo.stash_foreach(|index, message, oid| {
            out.push(Stash { index, message: message.to_owned(), oid: *oid });
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
            let refs = labels.iter().filter(|(o, _)| *o == oid).map(|(_, l)| l.clone()).collect();
            let author = c.author();
            rows.push(CommitRow {
                oid,
                short: short_id(oid),
                parents: c.parent_ids().collect(),
                summary: c.summary().ok().flatten().unwrap_or("").to_owned(),
                author: author.name().unwrap_or("").to_owned(),
                email: author.email().unwrap_or("").to_owned(),
                time: author.when().seconds(),
                refs,
            });
        }
        Ok((rows, truncated))
    }

    #[allow(clippy::type_complexity)]
    fn status(&self) -> Result<(Vec<FileStatus>, Vec<FileStatus>, Vec<FileStatus>)> {
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
                conflicted.push(FileStatus { path, old_path: None, kind: FileKind::Conflicted });
                continue;
            }
            if s.is_index_new() || s.is_index_modified() || s.is_index_deleted() || s.is_index_renamed() || s.is_index_typechange() {
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
                        let new = d.new_file().path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                        let old = d.old_file().path().map(|p| p.to_string_lossy().into_owned());
                        let old = old.filter(|o| kind == FileKind::Renamed && *o != new);
                        (new, old)
                    })
                    .unwrap_or((path.clone(), None));
                staged.push(FileStatus { path, old_path, kind });
            }
            if s.is_wt_new() || s.is_wt_modified() || s.is_wt_deleted() || s.is_wt_renamed() || s.is_wt_typechange() {
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
                        let new = d.new_file().path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                        let old = d.old_file().path().map(|p| p.to_string_lossy().into_owned());
                        let old = old.filter(|o| kind == FileKind::Renamed && *o != new);
                        (new, old)
                    })
                    .unwrap_or((path.clone(), None));
                unstaged.push(FileStatus { path, old_path, kind });
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
        let mut diff = self.repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;
        let mut find = git2::DiffFindOptions::new();
        find.renames(true);
        diff.find_similar(Some(&mut find))?;
        let mut out = Vec::new();
        for d in diff.deltas() {
            let new = d.new_file().path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let old = d.old_file().path().map(|p| p.to_string_lossy().into_owned());
            let kind = match d.status() {
                git2::Delta::Added => FileKind::Added,
                git2::Delta::Deleted => FileKind::Deleted,
                git2::Delta::Renamed => FileKind::Renamed,
                git2::Delta::Typechange => FileKind::TypeChange,
                _ => FileKind::Modified,
            };
            out.push(FileStatus { path: new.clone(), old_path: old.filter(|o| kind == FileKind::Renamed && *o != new), kind });
        }
        Ok(out)
    }

    /// Diff text for one file.
    pub fn diff(&self, target: &DiffTarget) -> Result<DiffText> {
        let path = target.path();
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path).include_untracked(true).recurse_untracked_dirs(true).show_untracked_content(true).context_lines(3);
        let diff = match target {
            DiffTarget::WorkdirUnstaged(_) => self.repo.diff_index_to_workdir(None, Some(&mut opts))?,
            DiffTarget::Staged(_) => {
                let head_tree = self.repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                let index = self.repo.index()?;
                self.repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))?
            }
            DiffTarget::Commit(oid, _) => {
                let commit = self.repo.find_commit(*oid)?;
                let tree = commit.tree()?;
                let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
                self.repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?
            }
        };
        let mut out = DiffText { target: target.clone(), binary: false, too_large: false, hunks: Vec::new() };
        let n = diff.deltas().len();
        for idx in 0..n {
            let delta = diff.get_delta(idx).expect("delta index in range");
            let size = delta.new_file().size().max(delta.old_file().size());
            if size > MAX_DIFF_BYTES {
                out.too_large = true;
                continue;
            }
            if delta.new_file().is_binary() || delta.old_file().is_binary() {
                out.binary = true;
                continue;
            }
            let Some(patch) = git2::Patch::from_diff(&diff, idx)? else { continue };
            for h in 0..patch.num_hunks() {
                let (hunk, count) = patch.hunk(h)?;
                let mut lines = Vec::with_capacity(count);
                for l in 0..count {
                    let line = patch.line_in_hunk(h, l)?;
                    let origin = line.origin();
                    if !matches!(origin, '+' | '-' | ' ') {
                        continue;
                    }
                    let mut text = String::from_utf8_lossy(line.content()).into_owned();
                    while text.ends_with('\n') || text.ends_with('\r') {
                        text.pop();
                    }
                    lines.push(DiffLine { origin, old_no: line.old_lineno(), new_no: line.new_lineno(), text });
                }
                let header = String::from_utf8_lossy(hunk.header()).trim_end().to_owned();
                out.hunks.push(Hunk { header, lines });
            }
        }
        Ok(out)
    }
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

        pub fn add(&self, rel: &str) {
            let mut idx = self.repo.index().unwrap();
            idx.add_path(Path::new(rel)).unwrap();
            idx.write().unwrap();
        }

        pub fn commit(&self, message: &str) -> git2::Oid {
            let mut idx = self.repo.index().unwrap();
            let tree_id = idx.write_tree().unwrap();
            let tree = self.repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("Test User", "test@example.com").unwrap();
            let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
            let parents: Vec<&git2::Commit> = parent.iter().collect();
            self.repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).unwrap()
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
    fn status_categories() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "one\n", "init");
        t.commit_file("del.txt", "x\n", "add del");
        t.commit_file("ren.txt", "same content for rename detection\nline2\nline3\n", "add ren");
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
        assert_eq!(find(&s.unstaged, "new.txt").unwrap().kind, FileKind::Untracked);
        assert_eq!(find(&s.staged, "added.txt").unwrap().kind, FileKind::Added);
        assert_eq!(find(&s.unstaged, "del.txt").unwrap().kind, FileKind::Deleted);
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

        let staged = r.diff(&DiffTarget::Staged("f.txt".into())).unwrap();
        assert_eq!(staged.hunks.len(), 1);
        let lines: Vec<(char, &str)> = staged.hunks[0].lines.iter().map(|l| (l.origin, l.text.as_str())).collect();
        assert_eq!(lines, vec![(' ', "a"), ('-', "b"), ('+', "B"), (' ', "c")]);
        assert_eq!(staged.hunks[0].lines[1].old_no, Some(2));
        assert_eq!(staged.hunks[0].lines[2].new_no, Some(2));
        assert!(staged.hunks[0].header.starts_with("@@ -1,3 +1,3 @@"));

        let unstaged = r.diff(&DiffTarget::WorkdirUnstaged("f.txt".into())).unwrap();
        let lines: Vec<(char, &str)> = unstaged.hunks[0].lines.iter().map(|l| (l.origin, l.text.as_str())).collect();
        assert_eq!(lines, vec![(' ', "a"), (' ', "B"), (' ', "c"), ('+', "d")]);

        let commit = r.diff(&DiffTarget::Commit(c1, "f.txt".into())).unwrap();
        assert_eq!(commit.hunks[0].lines.iter().filter(|l| l.origin == '+').count(), 3);
        let files = r.commit_files(c1).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].kind, FileKind::Added);

        // untracked file diff shows its content as additions
        t.write("u.txt", "x\ny\n");
        let u = r.diff(&DiffTarget::WorkdirUnstaged("u.txt".into())).unwrap();
        assert_eq!(u.hunks[0].lines.iter().filter(|l| l.origin == '+').count(), 2);
    }

    #[test]
    fn branches_tags_and_refs() {
        let t = TempRepo::new();
        let c1 = t.commit_file("a", "1", "one");
        let c2 = t.commit_file("a", "2", "two");
        t.repo.tag_lightweight("v1", &t.repo.find_object(c1, None).unwrap(), false).unwrap();
        t.repo.branch("feature", &t.repo.find_commit(c2).unwrap(), false).unwrap();
        let mut r = Repo::open(&t.dir).unwrap();
        let s = r.snapshot(100).unwrap();
        let names: Vec<&str> = s.branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"feature"));
        assert_eq!(s.tags, vec![Tag { name: "v1".into(), oid: c1 }]);
        let top = &s.commits[0];
        assert!(top.refs.iter().any(|l| l.name == "feature" && l.kind == RefKind::LocalBranch));
        assert!(s.commits[1].refs.iter().any(|l| l.name == "v1" && l.kind == RefKind::Tag));
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
}
