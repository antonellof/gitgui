//! Write operations beyond the basic stage / commit / checkout set:
//! cherry-pick, revert, merge, reset, tags, remotes, upstreams, conflict
//! resolution and partial (line level) staging. Everything runs on the
//! worker thread through git2. Operations that need the sequencer (rebase,
//! continue / abort of an in-progress operation) live in `ops.rs` and use the
//! git CLI, because libgit2 has no equivalent.

use std::path::Path;

use git2::Oid;

use super::repo::{DiffOpts, DiffTarget, DiffText, FileKind, Repo, Result};

/// What a merge, cherry-pick or revert produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    UpToDate,
    FastForward,
    Committed(Oid),
    /// Conflicts are left in the index for the user to resolve.
    Conflicts(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetKind {
    Soft,
    Mixed,
    Hard,
}

impl ResetKind {
    pub fn label(self) -> &'static str {
        match self {
            ResetKind::Soft => "soft",
            ResetKind::Mixed => "mixed",
            ResetKind::Hard => "hard",
        }
    }
}

/// Which side of a conflict to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    Ours,
    Theirs,
}

fn err(msg: impl AsRef<str>) -> super::repo::GitError {
    git2::Error::from_str(msg.as_ref()).into()
}

impl Repo {
    fn head_commit(&self) -> Result<git2::Commit<'_>> {
        Ok(self.repo.head()?.peel_to_commit()?)
    }

    fn conflict_count(&self) -> Result<usize> {
        // In-memory index on purpose: cherry-pick and revert leave their
        // result there before it is written.
        let index = self.repo.index()?;
        if !index.has_conflicts() {
            return Ok(0);
        }
        let mut paths = std::collections::HashSet::new();
        for c in index.conflicts()?.flatten() {
            let entry = c.our.or(c.their).or(c.ancestor);
            if let Some(e) = entry {
                paths.insert(e.path);
            }
        }
        Ok(paths.len().max(1))
    }

    /// Commit the index with explicit author and parents.
    fn commit_index(
        &self,
        message: &str,
        author: &git2::Signature<'_>,
        parents: &[&git2::Commit<'_>],
    ) -> Result<Oid> {
        let committer = self.repo.signature()?;
        let mut index = self.index()?;
        let tree_id = index.write_tree()?;
        let tree = self.repo.find_tree(tree_id)?;
        Ok(self
            .repo
            .commit(Some("HEAD"), author, &committer, message, &tree, parents)?)
    }

    /// libgit2's cherry-pick and revert leave their result in the in-memory
    /// index and their safe checkout neither creates files missing from the
    /// working tree nor deletes removed ones. Persist the index, then bring
    /// the paths the commit touched in line with the new HEAD. A forced
    /// checkout is fine there because the operation already refused to run
    /// when those paths had local changes.
    fn finish_pick(&self, picked: Oid, message: &str, author: &git2::Signature<'_>) -> Result<Oid> {
        let mut index = self.repo.index()?;
        index.write()?;
        let head = self.head_commit()?;
        let new = self.commit_index(message, author, &[&head])?;
        self.repo.cleanup_state()?;
        let files = self.commit_files(picked)?;
        let tree = self.head_commit()?.tree()?;
        let mut cb = git2::build::CheckoutBuilder::new();
        cb.force();
        let mut any = false;
        for path in files
            .iter()
            .flat_map(|f| std::iter::once(&f.path).chain(f.old_path.iter()))
        {
            if tree.get_path(Path::new(path)).is_ok() {
                cb.path(path);
                any = true;
            } else {
                let full = self.workdir.join(path);
                if full.is_file() {
                    std::fs::remove_file(&full).map_err(|e| err(e.to_string()))?;
                }
            }
        }
        if any {
            self.repo
                .checkout_tree(tree.as_object(), Some(&mut cb))?;
        }
        Ok(new)
    }

    fn git_cli(&self, args: &[&str]) -> Result<()> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.workdir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .output()
            .map_err(|e| err(format!("running git: {e}")))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(err(String::from_utf8_lossy(&out.stderr).trim()))
        }
    }

    /// Apply `oid` on top of HEAD as a new commit. Conflicts stay in the
    /// index with the cherry-pick state set, so `git cherry-pick --continue`
    /// finishes it.
    pub fn cherry_pick(&self, oid: Oid) -> Result<MergeOutcome> {
        if self.gpgsign() {
            self.git_cli(&["cherry-pick", &oid.to_string()])?;
            return Ok(MergeOutcome::Committed(self.head_commit()?.id()));
        }
        self.index()?;
        let commit = self.repo.find_commit(oid)?;
        let mut opts = git2::CherrypickOptions::new();
        if commit.parent_count() > 1 {
            opts.mainline(1);
        }
        self.repo.cherrypick(&commit, Some(&mut opts))?;
        let conflicts = self.conflict_count()?;
        if conflicts > 0 {
            return Ok(MergeOutcome::Conflicts(conflicts));
        }
        let message = commit.message().unwrap_or("").to_owned();
        let author = commit.author();
        let new = self.finish_pick(oid, &message, &author)?;
        Ok(MergeOutcome::Committed(new))
    }

    /// Create a commit that undoes `oid`.
    pub fn revert(&self, oid: Oid) -> Result<MergeOutcome> {
        if self.gpgsign() {
            self.git_cli(&["revert", "--no-edit", &oid.to_string()])?;
            return Ok(MergeOutcome::Committed(self.head_commit()?.id()));
        }
        self.index()?;
        let commit = self.repo.find_commit(oid)?;
        let mut opts = git2::RevertOptions::new();
        if commit.parent_count() > 1 {
            opts.mainline(1);
        }
        self.repo.revert(&commit, Some(&mut opts))?;
        let conflicts = self.conflict_count()?;
        if conflicts > 0 {
            return Ok(MergeOutcome::Conflicts(conflicts));
        }
        let summary = commit.summary().ok().flatten().unwrap_or("");
        let message = format!("Revert \"{summary}\"\n\nThis reverts commit {oid}.\n");
        let sig = self.repo.signature()?;
        let new = self.finish_pick(oid, &message, &sig)?;
        Ok(MergeOutcome::Committed(new))
    }

    fn find_branch_ref(&self, name: &str) -> Result<git2::Reference<'_>> {
        if let Ok(r) = self.repo.find_reference(&format!("refs/heads/{name}")) {
            return Ok(r);
        }
        if let Ok(r) = self.repo.find_reference(&format!("refs/remotes/{name}")) {
            return Ok(r);
        }
        Ok(self.repo.find_reference(name)?)
    }

    /// Merge branch `name` into the checked out branch.
    pub fn merge(&self, name: &str) -> Result<MergeOutcome> {
        if self.repo.head_detached().unwrap_or(false) {
            return Err(err("cannot merge into a detached HEAD"));
        }
        if self.gpgsign() {
            self.git_cli(&["merge", "--no-edit", name])?;
            return Ok(MergeOutcome::Committed(self.head_commit()?.id()));
        }
        self.index()?;
        let their_ref = self.find_branch_ref(name)?;
        let their = self.repo.reference_to_annotated_commit(&their_ref)?;
        let their_commit = their_ref.peel_to_commit()?;
        let (analysis, pref) = self.repo.merge_analysis(&[&their])?;
        if analysis.is_up_to_date() {
            return Ok(MergeOutcome::UpToDate);
        }
        if analysis.is_fast_forward() && !pref.is_no_fast_forward() {
            let obj = their_commit.as_object();
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.safe();
            self.repo.checkout_tree(obj, Some(&mut cb))?;
            let mut head = self.repo.head()?;
            head.set_target(their_commit.id(), &format!("fast-forward to {name}"))?;
            return Ok(MergeOutcome::FastForward);
        }
        if !analysis.is_normal() {
            return Err(err("merge is not possible here"));
        }
        let mut cb = git2::build::CheckoutBuilder::new();
        cb.safe();
        self.repo.merge(&[&their], None, Some(&mut cb))?;
        let conflicts = self.conflict_count()?;
        if conflicts > 0 {
            return Ok(MergeOutcome::Conflicts(conflicts));
        }
        let head = self.head_commit()?;
        let message = if their_ref.is_remote() {
            format!("Merge remote-tracking branch '{name}'")
        } else {
            format!("Merge branch '{name}'")
        };
        let sig = self.repo.signature()?;
        let new = self.commit_index(&message, &sig, &[&head, &their_commit])?;
        self.repo.cleanup_state()?;
        Ok(MergeOutcome::Committed(new))
    }

    /// Move the checked out branch to `oid`.
    pub fn reset(&self, oid: Oid, kind: ResetKind) -> Result<()> {
        self.index()?;
        let obj = self.repo.find_object(oid, None)?;
        let kind = match kind {
            ResetKind::Soft => git2::ResetType::Soft,
            ResetKind::Mixed => git2::ResetType::Mixed,
            ResetKind::Hard => git2::ResetType::Hard,
        };
        self.repo.reset(&obj, kind, None)?;
        Ok(())
    }

    /// Check out a commit as a detached HEAD.
    pub fn checkout_detached(&self, oid: Oid) -> Result<()> {
        self.index()?;
        let obj = self.repo.find_object(oid, None)?;
        let mut cb = git2::build::CheckoutBuilder::new();
        cb.safe();
        self.repo.checkout_tree(&obj, Some(&mut cb))?;
        self.repo.set_head_detached(oid)?;
        Ok(())
    }

    /// Create a tag; annotated when `message` is non-empty.
    pub fn create_tag(&self, name: &str, oid: Oid, message: &str) -> Result<()> {
        let obj = self.repo.find_object(oid, None)?;
        if message.trim().is_empty() {
            self.repo.tag_lightweight(name, &obj, false)?;
        } else {
            let sig = self.repo.signature()?;
            self.repo.tag(name, &obj, &sig, message.trim(), false)?;
        }
        Ok(())
    }

    pub fn delete_tag(&self, name: &str) -> Result<()> {
        self.repo.tag_delete(name)?;
        Ok(())
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        let mut b = self.repo.find_branch(old, git2::BranchType::Local)?;
        b.rename(new, false)?;
        Ok(())
    }

    /// Set (or clear with `None`) the upstream of a local branch. The
    /// upstream is a remote branch name like `origin/main`.
    pub fn set_upstream(&self, branch: &str, upstream: Option<&str>) -> Result<()> {
        let mut b = self.repo.find_branch(branch, git2::BranchType::Local)?;
        b.set_upstream(upstream)?;
        Ok(())
    }

    /// Advance a local branch to its upstream when the upstream is strictly
    /// ahead. Updates the working tree when the branch is checked out.
    pub fn fast_forward(&self, branch: &str) -> Result<usize> {
        let b = self.repo.find_branch(branch, git2::BranchType::Local)?;
        let local = b.get().target().ok_or_else(|| err("branch has no target"))?;
        let up = b.upstream().map_err(|_| err("branch has no upstream"))?;
        let target = up.get().target().ok_or_else(|| err("upstream has no target"))?;
        let (ahead, behind) = self.repo.graph_ahead_behind(local, target)?;
        if ahead > 0 {
            return Err(err(format!(
                "{branch} has diverged from its upstream ({ahead} ahead, {behind} behind)"
            )));
        }
        if behind == 0 {
            return Ok(0);
        }
        let is_head = self.head_info().and_then(|h| h.branch_name) == Some(branch.to_owned());
        if is_head {
            let obj = self.repo.find_object(target, None)?;
            let mut cb = git2::build::CheckoutBuilder::new();
            cb.safe();
            self.repo.checkout_tree(&obj, Some(&mut cb))?;
        }
        let mut r = b.into_reference();
        r.set_target(target, "fast-forward")?;
        Ok(behind)
    }

    /// Name of the checked out branch and its upstream, for push.
    pub fn current_branch_upstream(&self) -> Option<(String, Option<String>)> {
        let name = self.head_info()?.branch_name?;
        let b = self.repo.find_branch(&name, git2::BranchType::Local).ok()?;
        let up = b
            .upstream()
            .ok()
            .and_then(|u| u.name().ok().flatten().map(|s| s.to_owned()));
        Some((name, up))
    }

    pub fn remote_add(&self, name: &str, url: &str) -> Result<()> {
        self.repo.remote(name, url)?;
        Ok(())
    }

    pub fn remote_remove(&self, name: &str) -> Result<()> {
        self.repo.remote_delete(name)?;
        Ok(())
    }

    pub fn remote_rename(&self, old: &str, new: &str) -> Result<()> {
        self.repo.remote_rename(old, new)?;
        Ok(())
    }

    pub fn remote_set_url(&self, name: &str, url: &str) -> Result<()> {
        self.repo.remote_set_url(name, url)?;
        Ok(())
    }

    pub fn remote_url(&self, name: &str) -> Option<String> {
        let remote = self.repo.find_remote(name).ok()?;
        let url = remote.url().ok()?.to_owned();
        Some(url)
    }

    /// Create a branch at the commit a stash was taken from, check it out and
    /// apply the stash there (like `git stash branch`). The stash is dropped
    /// when the apply succeeds.
    pub fn branch_from_stash(&mut self, index: usize, name: &str) -> Result<()> {
        let mut oid = None;
        self.repo.stash_foreach(|i, _, o| {
            if i == index {
                oid = Some(*o);
            }
            true
        })?;
        let oid = oid.ok_or_else(|| err("no such stash"))?;
        {
            let stash_commit = self.repo.find_commit(oid)?;
            let base = stash_commit.parent(0)?;
            self.repo.branch(name, &base, false)?;
        }
        self.checkout(name, false)?;
        self.repo.stash_apply(index, None)?;
        self.repo.stash_drop(index)?;
        Ok(())
    }

    /// Append a pattern to the top level `.gitignore`.
    pub fn ignore(&self, pattern: &str) -> Result<()> {
        use std::io::Write as _;
        let path = self.workdir.join(".gitignore");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| err(e.to_string()))?;
        let mut line = String::new();
        if !existing.is_empty() && !existing.ends_with('\n') {
            line.push('\n');
        }
        line.push_str(pattern);
        line.push('\n');
        f.write_all(line.as_bytes())
            .map_err(|e| err(e.to_string()))?;
        Ok(())
    }

    /// Throw away every working tree and index change: tracked files go back
    /// to HEAD, untracked files are deleted.
    pub fn discard_all(&self) -> Result<()> {
        if let Ok(head) = self.repo.head().and_then(|h| h.peel(git2::ObjectType::Commit)) {
            self.repo.reset(&head, git2::ResetType::Hard, None)?;
        } else {
            let mut index = self.index()?;
            index.clear()?;
            index.write()?;
        }
        let (unstaged, _, _) = self.status()?;
        let untracked: Vec<String> = unstaged
            .iter()
            .filter(|f| f.kind == FileKind::Untracked)
            .map(|f| f.path.clone())
            .collect();
        if !untracked.is_empty() {
            self.discard(&untracked)?;
        }
        Ok(())
    }

    /// Resolve a conflicted path by taking one side, then stage it.
    pub fn resolve_conflict(&self, path: &str, side: ConflictSide) -> Result<()> {
        let mut index = self.index()?;
        let conflict = index.conflict_get(Path::new(path))?;
        let entry = match side {
            ConflictSide::Ours => conflict.our,
            ConflictSide::Theirs => conflict.their,
        };
        let full = self.workdir.join(path);
        match entry {
            Some(e) => {
                let blob = self.repo.find_blob(e.id)?;
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| err(e.to_string()))?;
                }
                std::fs::write(&full, blob.content()).map_err(|e| err(e.to_string()))?;
                index.remove_path(Path::new(path))?;
                index.add_path(Path::new(path))?;
            }
            None => {
                // That side deleted the file.
                if full.exists() {
                    std::fs::remove_file(&full).map_err(|e| err(e.to_string()))?;
                }
                index.remove_path(Path::new(path))?;
            }
        }
        index.write()?;
        Ok(())
    }

    /// Stage a subset of the lines of one hunk of the unstaged diff.
    pub fn stage_lines(&self, path: &str, hunk: usize, lines: &[usize], opts: DiffOpts) -> Result<()> {
        let d = self.diff(&DiffTarget::WorkdirUnstaged(path.to_owned()), opts)?;
        let patch = partial_patch(&d, hunk, lines, false).ok_or_else(|| err("no lines selected"))?;
        let diff = git2::Diff::from_buffer(&patch)?;
        self.repo.apply(&diff, git2::ApplyLocation::Index, None)?;
        Ok(())
    }

    /// Remove a subset of the lines of one hunk of the staged diff from the index.
    pub fn unstage_lines(&self, path: &str, hunk: usize, lines: &[usize], opts: DiffOpts) -> Result<()> {
        let d = self.diff(&DiffTarget::Staged(path.to_owned()), opts)?;
        let patch = partial_patch(&d, hunk, lines, true).ok_or_else(|| err("no lines selected"))?;
        let diff = git2::Diff::from_buffer(&patch)?;
        self.repo.apply(&diff, git2::ApplyLocation::Index, None)?;
        Ok(())
    }

    /// Undo a subset of the lines of one hunk of the unstaged diff in the
    /// working tree.
    pub fn discard_lines(&self, path: &str, hunk: usize, lines: &[usize], opts: DiffOpts) -> Result<()> {
        let d = self.diff(&DiffTarget::WorkdirUnstaged(path.to_owned()), opts)?;
        if d.status == FileKind::Added {
            return Err(err("discard the whole untracked file instead"));
        }
        let patch = partial_patch(&d, hunk, lines, true).ok_or_else(|| err("no lines selected"))?;
        let diff = git2::Diff::from_buffer(&patch)?;
        self.repo
            .apply(&diff, git2::ApplyLocation::WorkDir, None)?;
        Ok(())
    }

    /// Undo a whole hunk of the unstaged diff in the working tree.
    pub fn discard_hunk(&self, path: &str, hunk: usize, opts: DiffOpts) -> Result<()> {
        let d = self.diff(&DiffTarget::WorkdirUnstaged(path.to_owned()), opts)?;
        let all: Vec<usize> = d
            .hunks
            .get(hunk)
            .map(|h| (0..h.lines.len()).collect())
            .unwrap_or_default();
        self.discard_lines(path, hunk, &all, opts)
    }
}

/// Parse `@@ -a,b +c,d @@` into (old_start, new_start).
fn hunk_starts(header: &str) -> Option<(u32, u32)> {
    let mut it = header.split_whitespace();
    it.next()?;
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next().and_then(|n| n.parse::<u32>().ok());
    Some((start(old)?, start(new)?))
}

/// Build a unified diff containing only hunk `hunk_index` of `d` with just
/// the selected lines as changes. With `reverse == false` the patch goes in
/// the direction of `d` and applies to the old side (staging): unselected
/// removals become context, unselected additions are dropped. With `reverse
/// == true` the patch undoes the selected lines and applies to the new side
/// (unstaging, discarding): unselected additions become context, unselected
/// removals are dropped. Returns `None` when no selected line is a change.
pub fn partial_patch(
    d: &DiffText,
    hunk_index: usize,
    selected: &[usize],
    reverse: bool,
) -> Option<Vec<u8>> {
    let hunk = d.hunks.get(hunk_index)?;
    let (old_start, new_start) = hunk_starts(&hunk.header)?;
    let mut body = String::new();
    // Counts of the emitted patch's own old (preimage) and new sides.
    let mut pre_count = 0u32;
    let mut post_count = 0u32;
    let mut changes = 0usize;
    let all_selected = (0..hunk.lines.len()).all(|i| selected.contains(&i));
    for (i, l) in hunk.lines.iter().enumerate() {
        let keep = selected.contains(&i);
        let origin = match (l.origin, keep, reverse) {
            (' ', _, _) => ' ',
            ('+', true, false) => '+',
            ('-', true, false) => '-',
            ('+', true, true) => '-',
            ('-', true, true) => '+',
            ('+', false, false) => continue,
            ('-', false, false) => ' ',
            ('+', false, true) => ' ',
            ('-', false, true) => continue,
            _ => continue,
        };
        match origin {
            '+' => {
                post_count += 1;
                changes += 1;
            }
            '-' => {
                pre_count += 1;
                changes += 1;
            }
            _ => {
                pre_count += 1;
                post_count += 1;
            }
        }
        body.push(origin);
        body.push_str(&l.text);
        body.push('\n');
        if l.no_newline {
            body.push_str("\\ No newline at end of file\n");
        }
    }
    if changes == 0 {
        return None;
    }
    let path = d.target.path();
    let mut out = format!("diff --git a/{path} b/{path}\n");
    let creates = matches!(d.status, FileKind::Added | FileKind::Untracked) && !reverse;
    let deletes = d.status == FileKind::Deleted && all_selected && !reverse;
    let undo_create =
        matches!(d.status, FileKind::Added | FileKind::Untracked) && all_selected && reverse;
    let undo_delete = d.status == FileKind::Deleted && reverse;
    if creates || undo_delete {
        out.push_str("new file mode 100644\n--- /dev/null\n");
        out.push_str(&format!("+++ b/{path}\n"));
    } else if deletes || undo_create {
        out.push_str("deleted file mode 100644\n");
        out.push_str(&format!("--- a/{path}\n+++ /dev/null\n"));
    } else {
        out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    }
    // A single hunk keeps the original start of the side it applies to; the
    // other side has no earlier hunks shifting it, except the "0,0" form for
    // an empty preimage.
    let pre_start = if reverse { new_start } else { old_start };
    let (pre_start, post_start) = if pre_count == 0 {
        (0, pre_start.max(1))
    } else if post_count == 0 {
        (pre_start, 0)
    } else {
        (pre_start, pre_start)
    };
    out.push_str(&format!(
        "@@ -{pre_start},{pre_count} +{post_start},{post_count} @@\n"
    ));
    out.push_str(&body);
    Some(out.into_bytes())
}

/// Turn a remote URL into the https URL of its web page, for GitHub-style
/// hosts. `git@github.com:o/r.git` and `https://github.com/o/r.git` both
/// give `https://github.com/o/r`.
pub fn web_url(remote: &str) -> Option<String> {
    let s = remote.trim();
    let (host, path) = if let Some(rest) = s.strip_prefix("git@") {
        let (h, p) = rest.split_once(':')?;
        (h.to_owned(), p.to_owned())
    } else if let Some(rest) = s.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (h, p) = rest.split_once('/')?;
        (h.to_owned(), p.to_owned())
    } else {
        let rest = s
            .strip_prefix("https://")
            .or_else(|| s.strip_prefix("http://"))?;
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (h, p) = rest.split_once('/')?;
        (h.to_owned(), p.to_owned())
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

/// Web page of a commit on the remote's host.
pub fn commit_url(remote: &str, oid: Oid) -> Option<String> {
    let base = web_url(remote)?;
    Some(if base.contains("gitlab") {
        format!("{base}/-/commit/{oid}")
    } else {
        format!("{base}/commit/{oid}")
    })
}

/// Page to open a pull request for `branch` on the remote's host.
pub fn pull_request_url(remote: &str, branch: &str) -> Option<String> {
    let base = web_url(remote)?;
    Some(if base.contains("gitlab") {
        format!("{base}/-/merge_requests/new?merge_request%5Bsource_branch%5D={branch}")
    } else {
        format!("{base}/pull/new/{branch}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo::testutil::TempRepo;
    use crate::git::repo::{DiffOpts, DiffTarget, Repo, RepoState};

    fn main_branch(t: &TempRepo) -> String {
        t.repo.head().unwrap().shorthand().unwrap().to_owned()
    }

    #[test]
    fn cherry_pick_and_revert() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "a\n", "base");
        let main = main_branch(&t);
        let r = Repo::open(&t.dir).unwrap();
        r.create_branch("feature", t.repo.head().unwrap().target().unwrap(), true)
            .unwrap();
        let c = t.commit_file("b.txt", "b\n", "add b");
        r.checkout(&main, false).unwrap();
        assert!(!t.dir.join("b.txt").exists());
        t.commit_file("m.txt", "m\n", "main moves on");
        match r.cherry_pick(c).unwrap() {
            MergeOutcome::Committed(new) => {
                let commit = t.repo.find_commit(new).unwrap();
                assert_eq!(commit.summary().unwrap().unwrap(), "add b");
                assert_ne!(new, c);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(t.dir.join("b.txt").exists());
        assert_eq!(r.state(), RepoState::Clean);
        let head = t.repo.head().unwrap().target().unwrap();
        match r.revert(head).unwrap() {
            MergeOutcome::Committed(new) => {
                let commit = t.repo.find_commit(new).unwrap();
                assert!(commit.summary().unwrap().unwrap().starts_with("Revert \"add b\""));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!t.dir.join("b.txt").exists());
    }

    #[test]
    fn merge_fast_forward_normal_and_conflict() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "a\n", "base");
        let main = main_branch(&t);
        let base = t.repo.head().unwrap().target().unwrap();
        let mut r = Repo::open(&t.dir).unwrap();
        // Fast-forward: feature is ahead, main did not move.
        r.create_branch("ff", base, true).unwrap();
        t.commit_file("ff.txt", "x\n", "ff commit");
        r.checkout(&main, false).unwrap();
        assert_eq!(r.merge("ff").unwrap(), MergeOutcome::FastForward);
        assert!(t.dir.join("ff.txt").exists());
        assert_eq!(r.merge("ff").unwrap(), MergeOutcome::UpToDate);
        // Normal merge: both sides moved on different files.
        let tip = t.repo.head().unwrap().target().unwrap();
        r.create_branch("side", tip, true).unwrap();
        t.commit_file("side.txt", "s\n", "side commit");
        r.checkout(&main, false).unwrap();
        t.commit_file("main.txt", "m\n", "main commit");
        match r.merge("side").unwrap() {
            MergeOutcome::Committed(oid) => {
                let c = t.repo.find_commit(oid).unwrap();
                assert_eq!(c.parent_count(), 2);
                assert_eq!(c.summary().unwrap().unwrap(), "Merge branch 'side'");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(r.state(), RepoState::Clean);
        // Conflict: both edit a.txt.
        let tip = t.repo.head().unwrap().target().unwrap();
        r.create_branch("clash", tip, true).unwrap();
        t.commit_file("a.txt", "theirs\n", "their edit");
        r.checkout(&main, false).unwrap();
        t.commit_file("a.txt", "ours\n", "our edit");
        assert_eq!(r.merge("clash").unwrap(), MergeOutcome::Conflicts(1));
        assert_eq!(r.state(), RepoState::Merge);
        let s = r.snapshot(50).unwrap();
        assert_eq!(s.conflicted.len(), 1);
        assert_eq!(s.state, RepoState::Merge);
        let view = r
            .diff(&DiffTarget::WorkdirUnstaged("a.txt".into()), DiffOpts::default())
            .unwrap();
        assert_eq!(view.status, crate::git::repo::FileKind::Conflicted);
        assert!(view.hunks[0].lines.iter().any(|l| l.origin == '-' && l.text == "ours"));
        assert!(view.hunks[0].lines.iter().any(|l| l.origin == '+' && l.text == "theirs"));
        // Resolve by taking theirs, then the index is clean again.
        r.resolve_conflict("a.txt", ConflictSide::Theirs).unwrap();
        assert_eq!(
            std::fs::read_to_string(t.dir.join("a.txt")).unwrap(),
            "theirs\n"
        );
        let s = r.snapshot(50).unwrap();
        assert!(s.conflicted.is_empty());
        assert_eq!(s.staged.len(), 1);
    }

    #[test]
    fn reset_kinds() {
        let t = TempRepo::new();
        let c1 = t.commit_file("a.txt", "1\n", "one");
        t.commit_file("a.txt", "2\n", "two");
        let mut r = Repo::open(&t.dir).unwrap();
        r.reset(c1, ResetKind::Soft).unwrap();
        let s = r.snapshot(10).unwrap();
        assert_eq!(s.commits.len(), 1);
        assert_eq!(s.staged.len(), 1, "soft keeps the change staged");
        let c2 = r.commit("two again", false).unwrap();
        r.reset(c1, ResetKind::Mixed).unwrap();
        let s = r.snapshot(10).unwrap();
        assert!(s.staged.is_empty());
        assert_eq!(s.unstaged.len(), 1, "mixed leaves the change unstaged");
        r.reset(c2, ResetKind::Hard).unwrap();
        let s = r.snapshot(10).unwrap();
        assert!(!s.is_dirty());
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "2\n");
        r.reset(c1, ResetKind::Hard).unwrap();
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "1\n");
    }

    #[test]
    fn tags_detached_checkout_rename_upstream() {
        let t = TempRepo::new();
        let c1 = t.commit_file("a.txt", "1\n", "one");
        t.commit_file("a.txt", "2\n", "two");
        let mut r = Repo::open(&t.dir).unwrap();
        r.create_tag("light", c1, "").unwrap();
        r.create_tag("heavy", c1, "an annotated tag").unwrap();
        let s = r.snapshot(10).unwrap();
        assert_eq!(s.tags.len(), 2);
        assert!(s.tags.iter().all(|t| t.oid == c1));
        r.delete_tag("light").unwrap();
        assert_eq!(r.snapshot(10).unwrap().tags.len(), 1);

        r.checkout_detached(c1).unwrap();
        let h = r.head_info().unwrap();
        assert!(h.detached);
        assert_eq!(h.oid, Some(c1));
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "1\n");

        let main = t
            .repo
            .branches(Some(git2::BranchType::Local))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .0
            .name()
            .unwrap()
            .unwrap()
            .to_owned();
        r.checkout(&main, false).unwrap();
        r.rename_branch(&main, "renamed").unwrap();
        assert_eq!(
            r.head_info().unwrap().branch_name.as_deref(),
            Some("renamed")
        );

        t.repo
            .remote("origin", "https://example.invalid/repo.git")
            .unwrap();
        t.repo
            .reference("refs/remotes/origin/renamed", c1, false, "")
            .unwrap();
        r.set_upstream("renamed", Some("origin/renamed")).unwrap();
        let s = r.snapshot(10).unwrap();
        let b = s.branches.iter().find(|b| b.name == "renamed").unwrap();
        assert_eq!(b.upstream.as_deref(), Some("origin/renamed"));
        assert_eq!(b.ahead, 1);
        assert_eq!(
            r.current_branch_upstream(),
            Some(("renamed".into(), Some("origin/renamed".into())))
        );
        r.set_upstream("renamed", None).unwrap();
        let s = r.snapshot(10).unwrap();
        assert!(s.branches.iter().find(|b| b.name == "renamed").unwrap().upstream.is_none());
    }

    #[test]
    fn fast_forward_from_upstream() {
        let t = TempRepo::new();
        let c1 = t.commit_file("a.txt", "1\n", "one");
        let c2 = t.commit_file("a.txt", "2\n", "two");
        let main = main_branch(&t);
        let r = Repo::open(&t.dir).unwrap();
        t.repo
            .remote("origin", "https://example.invalid/repo.git")
            .unwrap();
        t.repo
            .reference(&format!("refs/remotes/origin/{main}"), c2, false, "")
            .unwrap();
        r.reset(c1, ResetKind::Hard).unwrap();
        r.set_upstream(&main, Some(&format!("origin/{main}"))).unwrap();
        assert_eq!(r.fast_forward(&main).unwrap(), 1);
        assert_eq!(t.repo.head().unwrap().target().unwrap(), c2);
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "2\n");
        assert_eq!(r.fast_forward(&main).unwrap(), 0);
        t.commit_file("a.txt", "3\n", "three");
        assert!(r.fast_forward(&main).is_err(), "diverged");
    }

    #[test]
    fn remotes_ignore_discard_all_and_stash_variants() {
        let t = TempRepo::new();
        t.commit_file("a.txt", "1\n", "one");
        let mut r = Repo::open(&t.dir).unwrap();
        r.remote_add("origin", "https://example.invalid/a.git").unwrap();
        r.remote_add("fork", "https://example.invalid/b.git").unwrap();
        r.remote_rename("fork", "upstream").unwrap();
        r.remote_set_url("upstream", "https://example.invalid/c.git")
            .unwrap();
        assert_eq!(
            r.remote_url("upstream").as_deref(),
            Some("https://example.invalid/c.git")
        );
        r.remote_remove("upstream").unwrap();
        let s = r.snapshot(10).unwrap();
        assert_eq!(s.remotes, vec!["origin".to_string()]);

        t.write("junk.log", "x\n");
        r.ignore("*.log").unwrap();
        let s = r.snapshot(10).unwrap();
        assert!(s.unstaged.iter().all(|f| f.path != "junk.log"));
        assert!(s.unstaged.iter().any(|f| f.path == ".gitignore"));
        r.ignore("build/").unwrap();
        assert_eq!(
            std::fs::read_to_string(t.dir.join(".gitignore")).unwrap(),
            "*.log\nbuild/\n"
        );
        t.add(".gitignore");
        t.commit("ignore");

        t.write("a.txt", "dirty\n");
        r.stage(&["a.txt".into()]).unwrap();
        t.write("a.txt", "dirtier\n");
        t.write("new.txt", "n\n");
        r.discard_all().unwrap();
        let s = r.snapshot(10).unwrap();
        assert!(!s.is_dirty(), "{:?} {:?}", s.unstaged, s.staged);
        assert_eq!(std::fs::read_to_string(t.dir.join("a.txt")).unwrap(), "1\n");
        assert!(!t.dir.join("new.txt").exists());
        assert!(t.dir.join("junk.log").exists(), "ignored files are kept");

        // Keep index: staged change survives the stash.
        t.write("a.txt", "staged\n");
        r.stage(&["a.txt".into()]).unwrap();
        t.write("b.txt", "unstaged\n");
        r.stash_push_opts("partial", true, true).unwrap();
        let s = r.snapshot(10).unwrap();
        assert_eq!(s.staged.len(), 1);
        assert!(s.unstaged.is_empty());
        assert!(!t.dir.join("b.txt").exists());
        // libgit2 refuses to apply onto a dirty index; commit the staged part.
        r.commit("staged part", false).unwrap();
        r.stash_apply(0).unwrap();
        assert!(t.dir.join("b.txt").exists());
        assert_eq!(r.snapshot(10).unwrap().stashes.len(), 1, "apply keeps the stash");
        r.stash_drop(0).unwrap();
        r.discard_all().unwrap();

        // Branch from stash lands on a new branch with the stash applied.
        t.write("c.txt", "c\n");
        r.stash_push("for branch").unwrap();
        r.branch_from_stash(0, "from-stash").unwrap();
        assert_eq!(
            r.head_info().unwrap().branch_name.as_deref(),
            Some("from-stash")
        );
        assert!(t.dir.join("c.txt").exists());
        assert!(r.snapshot(10).unwrap().stashes.is_empty());
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn stage_unstage_discard_lines() {
        let t = TempRepo::new();
        let base = numbered(10);
        t.commit_file("f.txt", &base, "init");
        let r = Repo::open(&t.dir).unwrap();
        // One hunk with two separate changes: line 3 replaced, line 8 added after.
        let modified = base
            .replace("line 3\n", "LINE 3\n")
            .replace("line 8\n", "line 8\nline 8b\n");
        t.write("f.txt", &modified);
        let opts = DiffOpts::default();
        let d = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), opts)
            .unwrap();
        assert_eq!(d.hunks.len(), 1);
        let lines = &d.hunks[0].lines;
        let idx_of = |o: char, text: &str| {
            lines
                .iter()
                .position(|l| l.origin == o && l.text == text)
                .unwrap()
        };
        let del3 = idx_of('-', "line 3");
        let add3 = idx_of('+', "LINE 3");
        let add8b = idx_of('+', "line 8b");

        // Stage only the line 3 replacement.
        r.stage_lines("f.txt", 0, &[del3, add3], opts).unwrap();
        let staged = r.diff(&DiffTarget::Staged("f.txt".into()), opts).unwrap();
        let staged_changes: Vec<(char, String)> = staged.hunks[0]
            .lines
            .iter()
            .filter(|l| l.origin != ' ')
            .map(|l| (l.origin, l.text.clone()))
            .collect();
        assert_eq!(
            staged_changes,
            vec![('-', "line 3".into()), ('+', "LINE 3".into())]
        );
        let unstaged = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), opts)
            .unwrap();
        let rest: Vec<(char, String)> = unstaged.hunks[0]
            .lines
            .iter()
            .filter(|l| l.origin != ' ')
            .map(|l| (l.origin, l.text.clone()))
            .collect();
        assert_eq!(rest, vec![('+', "line 8b".into())]);
        assert_eq!(
            std::fs::read_to_string(t.dir.join("f.txt")).unwrap(),
            modified,
            "working tree untouched"
        );

        // Unstage just the added LINE 3 (leaves the deletion staged).
        let staged_add = staged.hunks[0]
            .lines
            .iter()
            .position(|l| l.origin == '+')
            .unwrap();
        r.unstage_lines("f.txt", 0, &[staged_add], opts).unwrap();
        let staged = r.diff(&DiffTarget::Staged("f.txt".into()), opts).unwrap();
        let staged_changes: Vec<(char, String)> = staged.hunks[0]
            .lines
            .iter()
            .filter(|l| l.origin != ' ')
            .map(|l| (l.origin, l.text.clone()))
            .collect();
        assert_eq!(staged_changes, vec![('-', "line 3".into())]);

        // Discard the 8b addition from the working tree.
        let unstaged = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), opts)
            .unwrap();
        let (hi, li) = unstaged
            .hunks
            .iter()
            .enumerate()
            .find_map(|(hi, h)| {
                h.lines
                    .iter()
                    .position(|l| l.origin == '+' && l.text == "line 8b")
                    .map(|li| (hi, li))
            })
            .unwrap();
        r.discard_lines("f.txt", hi, &[li], opts).unwrap();
        let text = std::fs::read_to_string(t.dir.join("f.txt")).unwrap();
        assert!(!text.contains("line 8b"));
        assert!(text.contains("LINE 3"));
        let _ = add8b;
    }

    #[test]
    fn partial_patch_new_file_and_no_newline() {
        let t = TempRepo::new();
        t.commit_file("keep.txt", "k\n", "init");
        let r = Repo::open(&t.dir).unwrap();
        t.write("new.txt", "one\ntwo\nthree");
        let opts = DiffOpts::default();
        let d = r
            .diff(&DiffTarget::WorkdirUnstaged("new.txt".into()), opts)
            .unwrap();
        assert_eq!(d.status, FileKind::Added);
        assert!(d.hunks[0].lines[2].no_newline);
        let patch = partial_patch(&d, 0, &[0, 1, 2], false).unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(text.contains("--- /dev/null\n+++ b/new.txt\n"));
        assert!(text.contains("@@ -0,0 +1,3 @@\n+one\n+two\n+three\n\\ No newline at end of file\n"));
        // Stage only the first two lines of the new file.
        r.stage_lines("new.txt", 0, &[0, 1], opts).unwrap();
        let staged = r.diff(&DiffTarget::Staged("new.txt".into()), opts).unwrap();
        assert_eq!(staged.hunks[0].lines.len(), 2);
        let unstaged = r
            .diff(&DiffTarget::WorkdirUnstaged("new.txt".into()), opts)
            .unwrap();
        assert_eq!(
            unstaged.hunks[0]
                .lines
                .iter()
                .filter(|l| l.origin == '+')
                .count(),
            1
        );
        // Nothing selected that changes anything: None.
        assert!(partial_patch(&d, 0, &[], false).is_none());
        assert!(partial_patch(&d, 5, &[0], true).is_none());
    }

    #[test]
    fn discard_hunk_restores_lines() {
        let t = TempRepo::new();
        let base = numbered(30);
        t.commit_file("f.txt", &base, "init");
        let r = Repo::open(&t.dir).unwrap();
        let modified = base
            .replace("line 2\n", "LINE 2\n")
            .replace("line 28\n", "LINE 28\n");
        t.write("f.txt", &modified);
        let opts = DiffOpts::default();
        r.discard_hunk("f.txt", 1, opts).unwrap();
        let text = std::fs::read_to_string(t.dir.join("f.txt")).unwrap();
        assert!(text.contains("LINE 2\n"));
        assert!(text.contains("line 28\n"));
        assert!(!text.contains("LINE 28"));
    }

    #[test]
    fn diff_opts_context_and_whitespace() {
        let t = TempRepo::new();
        let base = numbered(30);
        t.commit_file("f.txt", &base, "init");
        let r = Repo::open(&t.dir).unwrap();
        t.write(
            "f.txt",
            &base
                .replace("line 2\n", "LINE 2\n")
                .replace("line 10\n", "line  10\n"),
        );
        let wide = DiffOpts {
            context: 10,
            ignore_whitespace: false,
        };
        let d = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), wide)
            .unwrap();
        assert_eq!(d.hunks.len(), 1, "wide context merges the hunks");
        let narrow = DiffOpts {
            context: 1,
            ignore_whitespace: false,
        };
        let d = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), narrow)
            .unwrap();
        assert_eq!(d.hunks.len(), 2);
        assert_eq!(d.hunks[0].lines.len(), 4, "one context line each side");
        let ws = DiffOpts {
            context: 1,
            ignore_whitespace: true,
        };
        let d = r
            .diff(&DiffTarget::WorkdirUnstaged("f.txt".into()), ws)
            .unwrap();
        assert_eq!(d.hunks.len(), 1, "whitespace-only change hidden");
        // Hunk staging follows the same options.
        r.stage_hunk("f.txt", 0, ws).unwrap();
        let staged = r.diff(&DiffTarget::Staged("f.txt".into()), ws).unwrap();
        assert!(staged.hunks[0].lines.iter().any(|l| l.text == "LINE 2"));
    }

    #[test]
    fn web_urls() {
        assert_eq!(
            web_url("git@github.com:owner/repo.git").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            web_url("https://github.com/owner/repo").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            web_url("ssh://git@gitlab.com/group/sub/repo.git").as_deref(),
            Some("https://gitlab.com/group/sub/repo")
        );
        assert_eq!(
            web_url("https://user@bitbucket.org/o/r.git/").as_deref(),
            Some("https://bitbucket.org/o/r")
        );
        assert!(web_url("/local/path").is_none());
        let oid = Oid::from_str("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert_eq!(
            commit_url("git@github.com:o/r.git", oid).unwrap(),
            "https://github.com/o/r/commit/0123456789abcdef0123456789abcdef01234567"
        );
        assert!(commit_url("git@gitlab.com:o/r.git", oid)
            .unwrap()
            .contains("/-/commit/"));
        assert_eq!(
            pull_request_url("https://github.com/o/r.git", "feat").unwrap(),
            "https://github.com/o/r/pull/new/feat"
        );
    }
}
