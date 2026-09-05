//! Top-level application state and layout (docs/SPEC.md section 4).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use git2::Oid;

use crate::git::actions::{ConflictSide, ResetKind};
use crate::git::ops::{Command, Reply, StateAction};
use crate::git::rebase::TodoAction;
use crate::git::repo::{DiffOpts, DiffTarget, DiffText, FileStatus, RepoSnapshot, RepoState};
use crate::ui::theme::Theme;
use crate::ui::{branch_picker, changes, diff, help, icons, log, row, sidebar, toolbar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    WorkingTree,
    /// Index into `snapshot.commits`.
    Commit(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Log,
    Detail,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// A confirmation or input dialog on top of everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Discard(Vec<String>),
    NewBranch {
        name: String,
        from: Oid,
        from_label: String,
        checkout: bool,
    },
    DeleteBranch(String),
    DropStash(usize),
    /// Pick a branch to switch to (click current branch in the status bar).
    BranchPicker {
        filter: String,
    },
    /// Uncommitted changes block checkout; ask how to proceed.
    CheckoutConfirm {
        target: String,
    },
    /// Create a GitHub repo with gh and push (no origin yet).
    PublishGithub {
        name: String,
        description: String,
        private: bool,
    },
    /// Yes / no before a command that is hard to undo.
    Confirm {
        title: &'static str,
        body: String,
        button: &'static str,
        cmd: Command,
    },
    /// One or two text fields that build a command.
    Input {
        kind: InputKind,
        value: String,
        extra: String,
    },
    /// Soft / mixed / hard reset of the current branch to a commit.
    Reset {
        oid: Oid,
        label: String,
    },
    /// Stash with options.
    StashOpts {
        message: String,
        keep_index: bool,
        include_untracked: bool,
    },
    /// Continue / abort / skip the in-progress operation.
    StateMenu,
    /// Keyboard reference.
    Help,
}

/// What an [`Modal::Input`] dialog is for. `value` is the first field,
/// `extra` the optional second one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    /// Tag name, optional annotation message.
    Tag { oid: Oid, label: String },
    RenameBranch { old: String },
    /// Multiline commit message.
    Reword { oid: Oid, is_root: bool },
    /// Remote name and URL.
    RemoteAdd,
    RemoteUrl { name: String },
    RemoteRename { old: String },
    /// Remote branch like `origin/main`.
    SetUpstream { branch: String },
    BranchFromStash { index: usize },
    /// Pattern for .gitignore.
    Ignore,
}

impl InputKind {
    pub fn title(&self) -> &'static str {
        match self {
            InputKind::Tag { .. } => "New tag",
            InputKind::RenameBranch { .. } => "Rename branch",
            InputKind::Reword { .. } => "Reword commit",
            InputKind::RemoteAdd => "Add remote",
            InputKind::RemoteUrl { .. } => "Remote URL",
            InputKind::RemoteRename { .. } => "Rename remote",
            InputKind::SetUpstream { .. } => "Set upstream",
            InputKind::BranchFromStash { .. } => "Branch from stash",
            InputKind::Ignore => "Ignore",
        }
    }

    /// (first field hint, second field hint or None).
    pub fn hints(&self) -> (&'static str, Option<&'static str>) {
        match self {
            InputKind::Tag { .. } => ("tag name", Some("message (optional, makes an annotated tag)")),
            InputKind::RenameBranch { .. } => ("new name", None),
            InputKind::Reword { .. } => ("commit message", None),
            InputKind::RemoteAdd => ("name", Some("url")),
            InputKind::RemoteUrl { .. } => ("url", None),
            InputKind::RemoteRename { .. } => ("new name", None),
            InputKind::SetUpstream { .. } => ("remote/branch", None),
            InputKind::BranchFromStash { .. } => ("branch name", None),
            InputKind::Ignore => ("pattern", None),
        }
    }

    pub fn multiline(&self) -> bool {
        matches!(self, InputKind::Reword { .. })
    }

    pub fn valid(&self, value: &str, extra: &str) -> bool {
        let v = value.trim();
        match self {
            InputKind::RemoteAdd => !v.is_empty() && !v.contains(' ') && !extra.trim().is_empty(),
            InputKind::Reword { .. } | InputKind::Ignore | InputKind::RemoteUrl { .. } => {
                !v.is_empty()
            }
            _ => !v.is_empty() && !v.contains(' '),
        }
    }

    pub fn command(&self, value: &str, extra: &str) -> Command {
        let v = value.trim().to_owned();
        match self {
            InputKind::Tag { oid, .. } => Command::CreateTag {
                name: v,
                oid: *oid,
                message: extra.trim().to_owned(),
            },
            InputKind::RenameBranch { old } => Command::RenameBranch {
                old: old.clone(),
                new: v,
            },
            InputKind::Reword { oid, is_root } => Command::RewriteCommit {
                oid: *oid,
                action: TodoAction::Reword,
                message: Some(v),
                is_root: *is_root,
            },
            InputKind::RemoteAdd => Command::RemoteAdd {
                name: v,
                url: extra.trim().to_owned(),
            },
            InputKind::RemoteUrl { name } => Command::RemoteSetUrl {
                name: name.clone(),
                url: v,
            },
            InputKind::RemoteRename { old } => Command::RemoteRename {
                old: old.clone(),
                new: v,
            },
            InputKind::SetUpstream { branch } => Command::SetUpstream {
                branch: branch.clone(),
                upstream: Some(v),
            },
            InputKind::BranchFromStash { index } => Command::BranchFromStash {
                index: *index,
                name: v,
            },
            InputKind::Ignore => Command::Ignore(v),
        }
    }
}

/// A range of lines selected in one hunk of the diff viewer. Indices are
/// into `Hunk::lines`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSel {
    pub hunk: usize,
    pub anchor: usize,
    pub end: usize,
}

impl LineSel {
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.end), self.anchor.max(self.end))
    }

    pub fn contains(&self, hunk: usize, line: usize) -> bool {
        let (a, b) = self.range();
        hunk == self.hunk && (a..=b).contains(&line)
    }

    pub fn lines(&self) -> Vec<usize> {
        let (a, b) = self.range();
        (a..=b).collect()
    }
}

/// Whether a commit can be rewritten with a rebase from the current branch:
/// it sits on the first-parent chain below HEAD with no merge in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteInfo {
    pub is_root: bool,
    pub is_head: bool,
    /// A plain (non-merge) commit exists right below, for squash / fixup / move down.
    pub has_older: bool,
}

pub struct NetLog {
    pub label: &'static str,
    pub lines: Vec<String>,
    pub running: bool,
    pub open: bool,
}

pub struct App {
    pub theme: Theme,
    pub snapshot: Arc<RepoSnapshot>,
    pub have_snapshot: bool,
    pub selection: Selection,
    pub focus: Pane,
    pub commit_files: HashMap<Oid, Vec<FileStatus>>,
    pub selected_file: Option<DiffTarget>,
    pub diff: Option<DiffText>,
    pub diff_loading: bool,
    pub filter: String,
    pub filter_active: bool,
    pub filter_focus_requested: bool,
    /// Visible commit indices after filtering.
    pub filtered: Vec<usize>,
    pub commit_msg: String,
    pub amend: bool,
    pub wrap: bool,
    pub toasts: Vec<Toast>,
    pub last_op: Option<String>,
    pub pending: Vec<Command>,
    pub scroll_to_selection: bool,
    pub frame_ms: f32,
    pub transport: &'static str,
    pub scale: f32,
    pub show_debug: bool,
    pub sidebar_selected: Option<String>,
    pub modal: Option<Modal>,
    pub net: NetLog,
    /// Set when the commit box should take keyboard focus this frame.
    pub focus_commit_msg: bool,
    /// Last known HEAD message, used when amend is toggled on.
    pub amend_loaded: bool,
    /// Pending write ops, to disable buttons while one runs.
    pub busy: usize,
    /// Where the Commit button was laid out last pass (tests click it).
    pub commit_button_rect: Option<egui::Rect>,
    pub commit_push_button_rect: Option<egui::Rect>,
    /// Set when the user clicks Quit or presses `q`.
    pub quit: bool,
    /// Opened directory is not a git repository yet.
    pub no_repo: bool,
    /// Path the user opened (may become a repo after init).
    pub repo_path: PathBuf,
    /// Context lines and whitespace handling, mirrored to the worker.
    pub diff_opts: DiffOpts,
    /// Search in the diff pane.
    pub diff_search: String,
    pub diff_search_active: bool,
    pub diff_search_focus: bool,
    /// Index into the current match list.
    pub diff_match: usize,
    /// Scroll the diff to the current match on the next frame.
    pub diff_jump: bool,
    /// Lines selected in the diff for line-level staging.
    pub line_sel: Option<LineSel>,
    /// A drag that started on a diff line extends the selection.
    pub line_drag: bool,
}

impl App {
    pub fn new(theme: Theme, transport: &'static str, scale: f32, repo_path: PathBuf) -> Self {
        Self {
            theme,
            snapshot: Arc::new(RepoSnapshot::default()),
            have_snapshot: false,
            selection: Selection::WorkingTree,
            focus: Pane::Log,
            commit_files: HashMap::new(),
            selected_file: None,
            diff: None,
            diff_loading: false,
            filter: String::new(),
            filter_active: false,
            filter_focus_requested: false,
            filtered: Vec::new(),
            commit_msg: String::new(),
            amend: false,
            wrap: false,
            toasts: Vec::new(),
            last_op: None,
            pending: Vec::new(),
            scroll_to_selection: false,
            frame_ms: 0.0,
            transport,
            scale,
            show_debug: false,
            sidebar_selected: None,
            modal: None,
            net: NetLog {
                label: "",
                lines: Vec::new(),
                running: false,
                open: false,
            },
            focus_commit_msg: false,
            amend_loaded: false,
            busy: 0,
            commit_button_rect: None,
            commit_push_button_rect: None,
            quit: false,
            no_repo: false,
            repo_path,
            diff_opts: DiffOpts::default(),
            diff_search: String::new(),
            diff_search_active: false,
            diff_search_focus: false,
            diff_match: 0,
            diff_jump: false,
            line_sel: None,
            line_drag: false,
        }
    }

    pub fn request_quit(&mut self) {
        self.quit = true;
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        self.last_op = Some(text.clone());
        self.toasts.push(Toast {
            text,
            error,
            at: Instant::now(),
        });
    }

    /// Row count in the log including the virtual working tree row.
    pub fn has_worktree_row(&self) -> bool {
        self.snapshot.is_dirty() || self.snapshot.commits.is_empty()
    }

    pub fn apply(&mut self, reply: Reply) {
        match reply {
            Reply::NoRepo(path) => {
                self.no_repo = true;
                self.repo_path = path;
                self.have_snapshot = false;
            }
            Reply::Snapshot(s) => {
                self.no_repo = false;
                let first = !self.have_snapshot;
                self.snapshot = s;
                self.have_snapshot = true;
                self.commit_files.clear();
                self.rebuild_filter();
                if first {
                    self.selection = if self.has_worktree_row() || self.snapshot.commits.is_empty()
                    {
                        Selection::WorkingTree
                    } else {
                        Selection::Commit(0)
                    };
                    self.on_selection_changed();
                } else {
                    // Keep the selection valid and refresh what it shows.
                    match self.selection {
                        Selection::WorkingTree if !self.has_worktree_row() => {
                            self.selection = Selection::Commit(0);
                            self.on_selection_changed();
                        }
                        Selection::Commit(i) if i >= self.snapshot.commits.len() => {
                            self.selection = if self.has_worktree_row() {
                                Selection::WorkingTree
                            } else {
                                Selection::Commit(0)
                            };
                            self.on_selection_changed();
                        }
                        Selection::WorkingTree => {
                            // File list changed: keep the file if still present, else pick first.
                            let still = self
                                .selected_file
                                .as_ref()
                                .is_some_and(|t| self.worktree_has(t));
                            if still {
                                if let Some(t) = self.selected_file.clone() {
                                    self.pending.push(Command::LoadDiff(t));
                                }
                            } else {
                                self.select_first_worktree_file();
                            }
                        }
                        Selection::Commit(_) => self.on_selection_changed(),
                    }
                }
            }
            Reply::Diff(Ok(d)) => {
                if self.selected_file.as_ref() == Some(&d.target) {
                    if self.diff.as_ref().map(|old| old.hunks.len()) != Some(d.hunks.len()) {
                        self.line_sel = None;
                    }
                    self.diff = Some(d);
                    self.diff_loading = false;
                }
            }
            Reply::Diff(Err(e)) => {
                self.diff_loading = false;
                self.toast(format!("diff failed: {e}"), true);
            }
            Reply::CommitFiles(oid, Ok(files)) => {
                let first = files.first().map(|f| f.path.clone());
                self.commit_files.insert(oid, files);
                if let Selection::Commit(i) = self.selection {
                    if self.snapshot.commits.get(i).map(|c| c.oid) == Some(oid) {
                        let keep = self
                            .selected_file
                            .as_ref()
                            .is_some_and(|t| matches!(t, DiffTarget::Commit(o, _) if *o == oid));
                        if !keep {
                            self.select_file(first.map(|p| DiffTarget::Commit(oid, p)));
                        }
                    }
                }
            }
            Reply::CommitFiles(_, Err(e)) => self.toast(format!("commit files failed: {e}"), true),
            Reply::Op { label, result } => {
                self.busy = self.busy.saturating_sub(1);
                match result {
                    Ok(msg) => {
                        if label == "commit" {
                            self.commit_msg.clear();
                            self.amend = false;
                            self.amend_loaded = false;
                        }
                        self.toast(msg, false);
                    }
                    Err(e) => self.toast(format!("{label} failed: {e}"), true),
                }
                if self.net.running && matches!(label, "fetch" | "pull" | "push" | "publish") {
                    self.net.running = false;
                }
            }
            Reply::NetStart(label) => {
                self.net.label = label;
                self.net.lines.clear();
                self.net.running = true;
                self.net.open = true;
            }
            Reply::NetLine(line) => {
                self.net.lines.push(line);
                if self.net.lines.len() > 2000 {
                    self.net.lines.drain(..1000);
                }
            }
            Reply::Error(e) => self.toast(e, true),
        }
    }

    /// Queue a write or network command.
    pub fn run(&mut self, cmd: Command) {
        if self.no_repo && !matches!(cmd, Command::InitRepo) {
            self.toast("not a git repository", true);
            return;
        }
        self.busy += 1;
        self.pending.push(cmd);
    }

    pub fn init_repo(&mut self) {
        self.run(Command::InitRepo);
    }

    /// The file the detail pane currently shows, as (path, staged?).
    pub fn selected_worktree_file(&self) -> Option<(String, bool)> {
        match &self.selected_file {
            Some(DiffTarget::WorkdirUnstaged(p)) => Some((p.clone(), false)),
            Some(DiffTarget::Staged(p)) => Some((p.clone(), true)),
            _ => None,
        }
    }

    pub fn stage_selected(&mut self) {
        if let Some((p, false)) = self.selected_worktree_file() {
            self.run(Command::Stage(vec![p]));
        }
    }

    pub fn unstage_selected(&mut self) {
        if let Some((p, true)) = self.selected_worktree_file() {
            self.run(Command::Unstage(vec![p]));
        }
    }

    pub fn commit_now(&mut self) {
        let msg = self.commit_msg.trim().to_owned();
        if msg.is_empty() {
            self.toast("commit message is empty", true);
            return;
        }
        if self.snapshot.staged.is_empty() && !self.amend {
            self.toast("nothing staged", true);
            return;
        }
        self.run(Command::Commit {
            message: msg,
            amend: self.amend,
        });
    }

    pub fn commit_and_push_now(&mut self) {
        let msg = self.commit_msg.trim().to_owned();
        if msg.is_empty() {
            self.toast("commit message is empty", true);
            return;
        }
        if self.snapshot.staged.is_empty() && !self.amend {
            self.toast("nothing staged", true);
            return;
        }
        self.run(Command::CommitAndPush {
            message: msg,
            amend: self.amend,
        });
    }

    /// The selected commit, if the selection is a commit.
    pub fn selected_commit(&self) -> Option<usize> {
        match self.selection {
            Selection::Commit(i) if i < self.snapshot.commits.len() => Some(i),
            _ => None,
        }
    }

    /// Whether commit `idx` can be rewritten by rebasing the current branch.
    pub fn rewrite_info(&self, idx: usize) -> Option<RewriteInfo> {
        let s = &self.snapshot;
        let head = s.head.as_ref()?;
        if head.detached || s.state != RepoState::Clean {
            return None;
        }
        let target = s.commits.get(idx)?;
        if target.parents.len() > 1 {
            return None;
        }
        let mut cur = head.oid?;
        let mut is_head = true;
        loop {
            let ci = s.commits.iter().position(|c| c.oid == cur)?;
            if ci == idx {
                let has_older = target
                    .parents
                    .first()
                    .and_then(|p| s.commits.iter().find(|c| c.oid == *p))
                    .is_some_and(|p| p.parents.len() <= 1);
                return Some(RewriteInfo {
                    is_root: target.parents.is_empty(),
                    is_head,
                    has_older,
                });
            }
            let c = &s.commits[ci];
            if c.parents.len() != 1 {
                return None;
            }
            cur = c.parents[0];
            is_head = false;
        }
    }

    /// Rebase refuses to start on a dirty tree; say so instead of failing later.
    fn clean_for_rebase(&mut self) -> bool {
        if self.snapshot.state != RepoState::Clean {
            self.toast("finish the current operation first", true);
            return false;
        }
        if self.snapshot.is_dirty() {
            self.toast("commit or stash your changes before rewriting history", true);
            return false;
        }
        true
    }

    pub fn confirm(&mut self, title: &'static str, body: String, button: &'static str, cmd: Command) {
        if self.busy > 0 {
            return;
        }
        self.modal = Some(Modal::Confirm {
            title,
            body,
            button,
            cmd,
        });
    }

    pub fn input(&mut self, kind: InputKind, value: String, extra: String) {
        if self.busy > 0 {
            return;
        }
        self.modal = Some(Modal::Input { kind, value, extra });
    }

    fn commit_label(&self, idx: usize) -> String {
        self.snapshot
            .commits
            .get(idx)
            .map(|c| format!("{} {}", c.short, c.summary))
            .unwrap_or_default()
    }

    pub fn commit_new_branch(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        self.modal = Some(Modal::NewBranch {
            name: String::new(),
            from: c.oid,
            from_label: c.short.clone(),
            checkout: true,
        });
    }

    pub fn commit_tag(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        let (oid, label) = (c.oid, self.commit_label(idx));
        self.input(InputKind::Tag { oid, label }, String::new(), String::new());
    }

    pub fn commit_cherry_pick(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        let oid = c.oid;
        let body = format!("Apply {} on top of HEAD as a new commit?", self.commit_label(idx));
        self.confirm("Cherry-pick", body, "Cherry-pick", Command::CherryPick(oid));
    }

    pub fn commit_revert(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        let oid = c.oid;
        let body = format!("Create a commit that undoes {}?", self.commit_label(idx));
        self.confirm("Revert", body, "Revert", Command::Revert(oid));
    }

    pub fn commit_reset(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        if self.busy > 0 {
            return;
        }
        self.modal = Some(Modal::Reset {
            oid: c.oid,
            label: self.commit_label(idx),
        });
    }

    pub fn commit_checkout_detached(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        let oid = c.oid;
        if self.snapshot.is_dirty() {
            self.toast("commit or stash your changes before checking out a commit", true);
            return;
        }
        self.run(Command::CheckoutDetached(oid));
    }

    /// Drop, squash, fixup, edit or move a commit through a rebase.
    pub fn commit_rewrite(&mut self, idx: usize, action: TodoAction) {
        let Some(info) = self.rewrite_info(idx) else {
            self.toast("only commits on the current branch below HEAD can be rewritten", true);
            return;
        };
        if !self.clean_for_rebase() {
            return;
        }
        let oid = self.snapshot.commits[idx].oid;
        let label = self.commit_label(idx);
        let cmd = Command::RewriteCommit {
            oid,
            action,
            message: None,
            is_root: info.is_root,
        };
        match action {
            TodoAction::Drop => {
                self.confirm("Drop commit", format!("Remove {label} from the branch? Later commits are replayed on top."), "Drop", cmd)
            }
            TodoAction::Squash => self.confirm(
                "Squash",
                format!("Squash {label} into the commit below it? Both messages are kept."),
                "Squash",
                cmd,
            ),
            TodoAction::Fixup => self.confirm(
                "Fixup",
                format!("Meld {label} into the commit below it and discard its message?"),
                "Fixup",
                cmd,
            ),
            _ => self.run(cmd),
        }
    }

    pub fn commit_reword(&mut self, idx: usize) {
        let Some(info) = self.rewrite_info(idx) else {
            self.toast("only commits on the current branch below HEAD can be reworded", true);
            return;
        };
        if info.is_head {
            // The HEAD commit needs no rebase: amend keeps everything else.
            let c = &self.snapshot.commits[idx];
            let mut msg = c.summary.clone();
            if !c.body.is_empty() {
                msg.push_str("\n\n");
                msg.push_str(&c.body);
            }
            self.commit_msg = msg;
            self.amend = true;
            self.amend_loaded = true;
            self.selection = Selection::WorkingTree;
            self.focus_commit_msg = true;
            self.focus = Pane::Detail;
            return;
        }
        if !self.clean_for_rebase() {
            return;
        }
        let c = &self.snapshot.commits[idx];
        let mut msg = c.summary.clone();
        if !c.body.is_empty() {
            msg.push_str("\n\n");
            msg.push_str(&c.body);
        }
        let oid = c.oid;
        self.input(
            InputKind::Reword {
                oid,
                is_root: info.is_root,
            },
            msg,
            String::new(),
        );
    }

    /// Squash the `fixup!` / `squash!` commits above `idx` into their targets.
    pub fn commit_autosquash(&mut self, idx: usize) {
        let Some(info) = self.rewrite_info(idx) else {
            self.toast("only commits on the current branch can be autosquashed", true);
            return;
        };
        if !self.clean_for_rebase() {
            return;
        }
        let oid = self.snapshot.commits[idx].oid;
        self.run(Command::Autosquash {
            oid,
            is_root: info.is_root,
        });
    }

    /// Commit the staged changes as `fixup! <summary of idx>`.
    pub fn commit_create_fixup(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        if self.snapshot.staged.is_empty() {
            self.toast("stage the changes for the fixup first", true);
            return;
        }
        let message = format!("fixup! {}", c.summary);
        self.run(Command::Commit {
            message,
            amend: false,
        });
    }

    pub fn commit_copy_hash(&mut self, ctx: &egui::Context, idx: usize) {
        if let Some(c) = self.snapshot.commits.get(idx) {
            ctx.copy_text(c.oid.to_string());
            self.toast(format!("copied {}", c.short), false);
        }
    }

    pub fn commit_copy_message(&mut self, ctx: &egui::Context, idx: usize) {
        if let Some(c) = self.snapshot.commits.get(idx) {
            let mut msg = c.summary.clone();
            if !c.body.is_empty() {
                msg.push_str("\n\n");
                msg.push_str(&c.body);
            }
            ctx.copy_text(msg);
            self.toast("copied commit message", false);
        }
    }

    /// URL of the first remote's web host, origin preferred.
    pub fn web_remote(&self) -> Option<&str> {
        let s = &self.snapshot;
        s.remote_urls
            .iter()
            .find(|(n, _)| n == "origin")
            .or_else(|| s.remote_urls.first())
            .map(|(_, u)| u.as_str())
    }

    pub fn commit_open_in_browser(&mut self, idx: usize) {
        let Some(c) = self.snapshot.commits.get(idx) else { return };
        let url = self
            .web_remote()
            .and_then(|r| crate::git::actions::commit_url(r, c.oid));
        match url {
            Some(u) => self.open_url(&u),
            None => self.toast("no web remote for this repository", true),
        }
    }

    pub fn open_pull_request(&mut self, branch: &str) {
        let url = self
            .web_remote()
            .and_then(|r| crate::git::actions::pull_request_url(r, branch));
        match url {
            Some(u) => self.open_url(&u),
            None => self.toast("no web remote for this repository", true),
        }
    }

    pub fn open_url(&mut self, url: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.toast(format!("opened {url}"), false),
            Err(e) => self.toast(format!("cannot open browser: {e}"), true),
        }
    }

    // ---- diff options, search and line selection ----

    fn push_diff_opts(&mut self) {
        self.pending.push(Command::SetDiffOpts(self.diff_opts));
        self.line_sel = None;
        if let Some(t) = self.selected_file.clone() {
            self.diff_loading = true;
            self.pending.push(Command::LoadDiff(t));
        }
    }

    pub fn change_diff_context(&mut self, delta: i32) {
        let cur = self.diff_opts.context as i32;
        let next = (cur + delta).clamp(0, DiffOpts::MAX_CONTEXT as i32) as u32;
        if next != self.diff_opts.context {
            self.diff_opts.context = next;
            self.push_diff_opts();
            self.toast(format!("{next} context lines"), false);
        }
    }

    pub fn toggle_whitespace(&mut self) {
        self.diff_opts.ignore_whitespace = !self.diff_opts.ignore_whitespace;
        self.push_diff_opts();
        self.toast(
            if self.diff_opts.ignore_whitespace {
                "ignoring whitespace"
            } else {
                "showing whitespace changes"
            },
            false,
        );
    }

    pub fn open_diff_search(&mut self) {
        self.diff_search_active = true;
        self.diff_search_focus = true;
        self.focus = Pane::Detail;
    }

    pub fn close_diff_search(&mut self) {
        self.diff_search_active = false;
        self.diff_search.clear();
        self.diff_match = 0;
    }

    pub fn diff_next_match(&mut self, dir: i32) {
        if self.diff_search.is_empty() {
            return;
        }
        let n = diff::match_count(self);
        if n == 0 {
            self.toast("no matches", true);
            return;
        }
        let cur = self.diff_match as i32;
        self.diff_match = (cur + dir).rem_euclid(n as i32) as usize;
        self.diff_jump = true;
    }

    /// Selected lines, with the hunk they belong to, when they can be acted on.
    fn line_action_target(&self) -> Option<(String, bool, usize, Vec<usize>)> {
        let sel = self.line_sel?;
        let d = self.diff.as_ref()?;
        let (path, unstaged) = match &d.target {
            DiffTarget::WorkdirUnstaged(p) => (p.clone(), true),
            DiffTarget::Staged(p) => (p.clone(), false),
            DiffTarget::Commit(..) => return None,
        };
        let hunk = d.hunks.get(sel.hunk)?;
        let lines: Vec<usize> = sel
            .lines()
            .into_iter()
            .filter(|i| hunk.lines.get(*i).is_some_and(|l| l.origin != ' '))
            .collect();
        if lines.is_empty() {
            return None;
        }
        Some((path, unstaged, sel.hunk, lines))
    }

    /// True when a line selection with changes exists in a working tree diff.
    pub fn has_line_selection(&self) -> bool {
        self.line_action_target().is_some()
    }

    pub fn stage_selected_lines(&mut self) -> bool {
        match self.line_action_target() {
            Some((path, true, hunk_index, lines)) => {
                self.run(Command::StageLines {
                    path,
                    hunk_index,
                    lines,
                });
                self.line_sel = None;
                true
            }
            _ => false,
        }
    }

    pub fn unstage_selected_lines(&mut self) -> bool {
        match self.line_action_target() {
            Some((path, false, hunk_index, lines)) => {
                self.run(Command::UnstageLines {
                    path,
                    hunk_index,
                    lines,
                });
                self.line_sel = None;
                true
            }
            _ => false,
        }
    }

    pub fn discard_selected_lines(&mut self) -> bool {
        match self.line_action_target() {
            Some((path, true, hunk_index, lines)) => {
                let n = lines.len();
                self.confirm(
                    "Discard lines",
                    format!("Throw away {n} changed line{} of {path}? This cannot be undone.", if n == 1 { "" } else { "s" }),
                    "Discard",
                    Command::DiscardLines {
                        path,
                        hunk_index,
                        lines,
                    },
                );
                true
            }
            _ => false,
        }
    }

    // ---- working tree ----

    pub fn toggle_stage_selected(&mut self) {
        match self.selected_worktree_file() {
            Some((p, false)) => self.run(Command::Stage(vec![p])),
            Some((p, true)) => self.run(Command::Unstage(vec![p])),
            None => {}
        }
    }

    pub fn discard_selected(&mut self) {
        if let Some((p, false)) = self.selected_worktree_file() {
            if self.busy == 0 {
                self.modal = Some(Modal::Discard(vec![p]));
            }
        }
    }

    pub fn discard_all(&mut self) {
        let s = &self.snapshot;
        if !s.is_dirty() {
            return;
        }
        let body = format!(
            "Reset the index and working tree to HEAD and delete untracked files? {} unstaged, {} staged, {} conflicted. This cannot be undone.",
            s.unstaged.len(),
            s.staged.len(),
            s.conflicted.len()
        );
        self.confirm("Discard all changes", body, "Discard everything", Command::DiscardAll);
    }

    pub fn ignore_selected(&mut self) {
        let Some((p, false)) = self.selected_worktree_file() else { return };
        let untracked = self
            .snapshot
            .unstaged
            .iter()
            .any(|f| f.path == p && f.kind == crate::git::repo::FileKind::Untracked);
        if !untracked {
            self.toast("only untracked files can be ignored", true);
            return;
        }
        self.input(InputKind::Ignore, format!("/{p}"), String::new());
    }

    pub fn resolve_selected(&mut self, side: ConflictSide) {
        if let Some((p, false)) = self.selected_worktree_file() {
            self.run(Command::Resolve { path: p, side });
        }
    }

    pub fn state_action(&mut self, action: StateAction) {
        let Some(sub) = self.snapshot.state.git_subcommand() else {
            self.toast("no operation in progress", true);
            return;
        };
        if action == StateAction::Continue && !self.snapshot.conflicted.is_empty() {
            self.toast(
                format!("{} conflicted file(s) left, resolve them first", self.snapshot.conflicted.len()),
                true,
            );
            return;
        }
        let cmd = Command::State {
            action,
            subcommand: sub,
        };
        if action == StateAction::Abort {
            self.confirm(
                "Abort",
                format!("Abort the {} and go back to where it started?", self.snapshot.state.label()),
                "Abort",
                cmd,
            );
        } else {
            self.modal = None;
            self.run(cmd);
        }
    }

    pub fn open_state_menu(&mut self) {
        if self.snapshot.state != RepoState::Clean && self.busy == 0 {
            self.modal = Some(Modal::StateMenu);
        }
    }

    pub fn open_stash_dialog(&mut self) {
        if self.snapshot.is_dirty() && self.busy == 0 {
            self.modal = Some(Modal::StashOpts {
                message: String::new(),
                keep_index: false,
                include_untracked: true,
            });
        }
    }

    pub fn open_help(&mut self) {
        self.modal = Some(Modal::Help);
    }

    pub fn open_branch_picker(&mut self) {
        if self.busy > 0 {
            return;
        }
        self.modal = Some(Modal::BranchPicker {
            filter: String::new(),
        });
    }

    pub fn open_publish_github(&mut self) {
        if self.busy > 0 || branch_picker::has_origin(&self.snapshot) {
            return;
        }
        self.modal = Some(Modal::PublishGithub {
            name: branch_picker::default_github_repo_name(&self.snapshot),
            description: String::new(),
            private: false,
        });
    }

    fn valid_github_repo_name(name: &str) -> bool {
        let n = name.trim();
        !n.is_empty()
            && !n.contains(' ')
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/' || c == '.')
    }

    pub fn try_switch_branch(&mut self, target: String) {
        if self.busy > 0 {
            return;
        }
        let current = self
            .snapshot
            .head
            .as_ref()
            .and_then(|h| h.branch_name.clone());
        if current.as_deref() == Some(target.as_str()) {
            self.modal = None;
            return;
        }
        if self.snapshot.is_dirty() {
            self.modal = Some(Modal::CheckoutConfirm { target });
        } else {
            self.modal = None;
            self.run(Command::Checkout(target));
        }
    }

    fn stash_message_for_switch(&self, target: &str) -> String {
        let cur = self
            .snapshot
            .head
            .as_ref()
            .and_then(|h| h.branch_name.as_deref())
            .unwrap_or("HEAD");
        format!("WIP on {cur} before switching to {target}")
    }

    fn worktree_has(&self, t: &DiffTarget) -> bool {
        match t {
            DiffTarget::WorkdirUnstaged(p) => self
                .snapshot
                .unstaged
                .iter()
                .chain(self.snapshot.conflicted.iter())
                .any(|f| &f.path == p),
            DiffTarget::Staged(p) => self.snapshot.staged.iter().any(|f| &f.path == p),
            DiffTarget::Commit(..) => false,
        }
    }

    pub fn rebuild_filter(&mut self) {
        let q = self.filter.trim().to_lowercase();
        self.filtered = if q.is_empty() {
            (0..self.snapshot.commits.len()).collect()
        } else {
            self.snapshot
                .commits
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.summary.to_lowercase().contains(&q)
                        || c.author.to_lowercase().contains(&q)
                        || c.short.starts_with(&q)
                })
                .map(|(i, _)| i)
                .collect()
        };
        // Keep the selection on a visible row: a commit the filter hid, or
        // the working tree row while a filter is active, would otherwise make
        // the next j / k jump to the top.
        let rows = self.log_rows();
        if !rows.contains(&self.selection) {
            if let Some(first) = rows.first().copied() {
                self.select(first);
                self.scroll_to_selection = true;
            }
        }
    }

    pub fn select(&mut self, sel: Selection) {
        if sel != self.selection {
            self.selection = sel;
            self.on_selection_changed();
        }
    }

    pub fn on_selection_changed(&mut self) {
        self.diff = None;
        self.selected_file = None;
        match self.selection {
            Selection::WorkingTree => self.select_first_worktree_file(),
            Selection::Commit(i) => {
                if let Some(c) = self.snapshot.commits.get(i) {
                    let oid = c.oid;
                    if let Some(files) = self.commit_files.get(&oid) {
                        let first = files
                            .first()
                            .map(|f| DiffTarget::Commit(oid, f.path.clone()));
                        self.select_file(first);
                    } else {
                        self.pending.push(Command::LoadCommitFiles(oid));
                    }
                }
            }
        }
    }

    fn select_first_worktree_file(&mut self) {
        let s = &self.snapshot;
        let first = s
            .unstaged
            .first()
            .map(|f| DiffTarget::WorkdirUnstaged(f.path.clone()))
            .or_else(|| s.staged.first().map(|f| DiffTarget::Staged(f.path.clone())))
            .or_else(|| {
                s.conflicted
                    .first()
                    .map(|f| DiffTarget::WorkdirUnstaged(f.path.clone()))
            });
        self.select_file(first);
    }

    pub fn select_file(&mut self, target: Option<DiffTarget>) {
        if self.selected_file != target {
            self.line_sel = None;
        }
        self.selected_file = target.clone();
        self.diff = None;
        if let Some(t) = target {
            self.diff_loading = true;
            self.pending.push(Command::LoadDiff(t));
        } else {
            self.diff_loading = false;
        }
    }

    /// Move the log selection by `delta` rows (keyboard navigation).
    pub fn move_selection(&mut self, delta: i32) {
        let rows = self.log_rows();
        if rows.is_empty() {
            return;
        }
        let cur = rows.iter().position(|r| *r == self.selection).unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, rows.len() as i32 - 1) as usize;
        self.select(rows[next]);
        self.scroll_to_selection = true;
    }

    /// The rows the log shows, in order.
    pub fn log_rows(&self) -> Vec<Selection> {
        let mut rows = Vec::with_capacity(self.filtered.len() + 1);
        if self.has_worktree_row() && self.filter.trim().is_empty() {
            rows.push(Selection::WorkingTree);
        }
        rows.extend(self.filtered.iter().map(|i| Selection::Commit(*i)));
        rows
    }

    pub fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            // A text field owns the keyboard. Ctrl+Enter commits from the
            // commit box, Ctrl+Shift+Enter commits and pushes, Escape leaves.
            if ctx.input(|i| ctrl(i, egui::Key::Enter)) && self.modal.is_none() {
                if ctx.input(|i| i.modifiers.shift) {
                    self.commit_and_push_now();
                } else {
                    self.commit_now();
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ctx.memory_mut(|m| m.request_focus(egui::Id::NULL));
                if self.filter_active {
                    self.filter_active = false;
                    self.filter.clear();
                    self.rebuild_filter();
                }
                if self.diff_search_active && self.diff_search.is_empty() {
                    self.close_diff_search();
                }
            }
            return;
        }
        if self.modal.is_some() {
            // Dialogs handle their own keys.
            return;
        }
        if ctx.input(|i| key_press(i, egui::Key::Questionmark).is_some()) {
            self.open_help();
            return;
        }
        let (down, up, pgdn, pgup, home, end, slash, esc, tab, r, dbg) = ctx.input(|i| {
            (
                plain(i, egui::Key::J) || plain(i, egui::Key::ArrowDown),
                plain(i, egui::Key::K) || plain(i, egui::Key::ArrowUp),
                plain(i, egui::Key::PageDown),
                plain(i, egui::Key::PageUp),
                plain(i, egui::Key::Home),
                plain(i, egui::Key::End),
                key_press(i, egui::Key::Slash).is_some(),
                plain(i, egui::Key::Escape),
                plain(i, egui::Key::Tab),
                plain(i, egui::Key::R),
                ctrl(i, egui::Key::D),
            )
        });
        let (s_key, u_key, a_key, shift_a, shift_s, c_key, f_key, p_key, shift_p, enter) = ctx.input(|i| {
            (
                plain(i, egui::Key::S),
                plain(i, egui::Key::U),
                plain(i, egui::Key::A),
                shifted(i, egui::Key::A),
                shifted(i, egui::Key::S),
                plain(i, egui::Key::C),
                plain(i, egui::Key::F),
                plain(i, egui::Key::P),
                shifted(i, egui::Key::P),
                plain(i, egui::Key::Enter),
            )
        });
        let (space, d_key, shift_d, i_key, n_key, shift_n, t_key, shift_t, shift_c, g_key) =
            ctx.input(|i| {
                (
                    plain(i, egui::Key::Space),
                    plain(i, egui::Key::D),
                    shifted(i, egui::Key::D),
                    plain(i, egui::Key::I),
                    plain(i, egui::Key::N),
                    shifted(i, egui::Key::N),
                    plain(i, egui::Key::T),
                    shifted(i, egui::Key::T),
                    shifted(i, egui::Key::C),
                    plain(i, egui::Key::G),
                )
            });
        let (shift_r, shift_k, shift_j, y_key, o_key, m_key, ctrl_f, ctrl_w, ctx_less, ctx_more) =
            ctx.input(|i| {
                (
                    shifted(i, egui::Key::R),
                    shifted(i, egui::Key::K),
                    shifted(i, egui::Key::J),
                    plain(i, egui::Key::Y),
                    plain(i, egui::Key::O),
                    plain(i, egui::Key::M),
                    ctrl(i, egui::Key::F),
                    ctrl(i, egui::Key::W),
                    key_press(i, egui::Key::OpenCurlyBracket).is_some(),
                    key_press(i, egui::Key::CloseCurlyBracket).is_some(),
                )
            });
        let commit = self.selected_commit();
        let searching = !self.diff_search.is_empty();
        if s_key && !self.stage_selected_lines() {
            self.stage_selected();
        }
        if u_key && !self.unstage_selected_lines() {
            self.unstage_selected();
        }
        if space {
            self.toggle_stage_selected();
        }
        if a_key {
            self.run(Command::StageAll);
        }
        if shift_a {
            self.run(Command::UnstageAll);
        }
        if shift_s {
            self.open_stash_dialog();
        }
        if shift_d {
            self.discard_all();
        }
        if i_key {
            self.ignore_selected();
        }
        if d_key {
            match commit {
                Some(idx) => self.commit_rewrite(idx, TodoAction::Drop),
                None => {
                    if !self.discard_selected_lines() {
                        self.discard_selected();
                    }
                }
            }
        }
        if n_key {
            if searching {
                self.diff_next_match(1);
            } else if let Some(idx) = commit {
                self.commit_new_branch(idx);
            }
        }
        if shift_n && searching {
            self.diff_next_match(-1);
        }
        if let Some(idx) = commit {
            if shift_t {
                self.commit_tag(idx);
            }
            if t_key {
                self.commit_revert(idx);
            }
            if shift_c {
                self.commit_cherry_pick(idx);
            }
            if g_key {
                self.commit_reset(idx);
            }
            if shift_r {
                self.commit_reword(idx);
            }
            if shift_k {
                self.commit_rewrite(idx, TodoAction::MoveUp);
            }
            if shift_j {
                self.commit_rewrite(idx, TodoAction::MoveDown);
            }
            if y_key {
                self.commit_copy_hash(ctx, idx);
            }
            if o_key {
                self.commit_open_in_browser(idx);
            }
        }
        if m_key {
            self.open_state_menu();
        }
        if ctrl_f {
            self.open_diff_search();
        }
        if ctrl_w {
            self.toggle_whitespace();
        }
        if ctx_less {
            self.change_diff_context(-1);
        }
        if ctx_more {
            self.change_diff_context(1);
        }
        if c_key {
            self.focus_commit_msg = true;
            self.selection = Selection::WorkingTree;
            self.focus = Pane::Detail;
        }
        if f_key {
            self.run(Command::Fetch);
        }
        if p_key {
            self.run(Command::Pull);
        }
        if shift_p {
            self.run(Command::Push);
        }
        if enter && self.focus == Pane::Sidebar {
            if let Some(name) = self.sidebar_selected.clone() {
                if self.snapshot.branches.iter().any(|b| b.name == name) {
                    self.run(Command::Checkout(name));
                }
            }
        }
        if self.focus == Pane::Log || self.focus == Pane::Sidebar {
            if down {
                self.move_selection(1);
            }
            if up {
                self.move_selection(-1);
            }
            if pgdn {
                self.move_selection(20);
            }
            if pgup {
                self.move_selection(-20);
            }
            if home {
                self.move_selection(-1_000_000);
            }
            if end {
                self.move_selection(1_000_000);
            }
        }
        if slash {
            self.filter_active = true;
            self.filter_focus_requested = true;
            self.focus = Pane::Log;
        }
        if esc {
            if self.line_sel.is_some() {
                self.line_sel = None;
            } else if self.diff_search_active {
                self.close_diff_search();
            } else if self.filter_active {
                self.filter_active = false;
                self.filter.clear();
                self.rebuild_filter();
            }
        }
        if tab {
            self.focus = match self.focus {
                Pane::Sidebar => Pane::Log,
                Pane::Log => Pane::Detail,
                Pane::Detail => Pane::Sidebar,
            };
        }
        if r {
            self.pending.push(Command::Refresh);
            self.toast("refreshing", false);
        }
        if dbg {
            self.show_debug = !self.show_debug;
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        if !self.no_repo {
            self.handle_keys(&ctx);
        } else {
            self.handle_keys_no_repo(&ctx);
        }
        self.toasts
            .retain(|t| t.at.elapsed().as_secs_f32() < if t.error { 8.0 } else { 3.0 });
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        if self.no_repo {
            self.show_no_repo(root);
            self.show_toasts(&ctx);
            return;
        }

        let sidebar_w = (root.available_width() * 0.25).clamp(140.0, 220.0);
        egui::Panel::left("sidebar")
            .default_size(sidebar_w)
            .resizable(true)
            .show(root, |ui| {
                sidebar::show(self, ui);
            });
        egui::Panel::bottom("status")
            .default_size(28.0)
            .resizable(false)
            .show(root, |ui| self.status_bar(ui));
        if self.net.open {
            egui::Panel::bottom("netlog")
                .default_size(140.0)
                .resizable(true)
                .show(root, |ui| self.net_log(ui));
        }

        let avail_h = root.available_height();
        egui::Panel::bottom("detail")
            .default_size(avail_h * 0.45)
            .resizable(true)
            .show(root, |ui| {
                changes::show_detail(self, ui);
            });
        egui::CentralPanel::default().show(root, |ui| {
            log::show(self, ui);
        });

        self.show_toasts(&ctx);
        if let Some(cmd) = diff::take_pending(self) {
            self.run(cmd);
        }
        self.show_modal(&ctx);
    }

    fn show_no_repo(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .default_size(28.0)
            .resizable(false)
            .show(root, |ui| self.status_bar_no_repo(ui));
        egui::CentralPanel::default().show(root, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.28);
                ui.heading("Not a git repository");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(self.repo_path.display().to_string()).monospace(),
                );
                ui.add_space(16.0);
                ui.label("Initialize a repository here to start using gitgui.");
                ui.add_space(12.0);
                let busy = self.busy > 0;
                if ui
                    .add_enabled(!busy, egui::Button::new("Initialize git repository"))
                    .on_hover_text("Runs git init in this folder")
                    .clicked()
                {
                    self.init_repo();
                }
            });
        });
    }

    fn status_bar_no_repo(&mut self, ui: &mut egui::Ui) {
        let repo_path = self.repo_path.clone();
        row::split(
            ui,
            |ui| {
                toolbar::show(self, ui);
                ui.separator();
            },
            |ui| {
                let path = repo_path.to_string_lossy();
                let shown = match std::env::var("HOME") {
                    Ok(h) if path.starts_with(&h) => format!("~{}", &path[h.len()..]),
                    _ => path.to_string(),
                };
                ui.label(shown.trim_end_matches('/').to_owned());
                ui.separator();
                ui.weak("no git repository");
            },
        );
    }

    fn handle_keys_no_repo(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| plain(i, egui::Key::Q)) {
            self.request_quit();
        }
    }

    fn net_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(if self.net.label == "publish" {
                format!("gh {}", self.net.label)
            } else {
                format!("git {}", self.net.label)
            });
            if self.net.running {
                ui.spinner();
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("close").clicked() {
                    self.net.open = false;
                }
            });
        });
        egui::ScrollArea::vertical()
            .id_salt("netlog_scroll")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for l in &self.net.lines {
                    ui.monospace(l);
                }
            });
    }

    fn show_modal(&mut self, ctx: &egui::Context) {
        let Some(modal) = self.modal.clone() else {
            return;
        };
        let mut close = false;
        let mut cmd: Option<Command> = None;
        let mut switch_branch: Option<String> = None;
        let mut open_new_branch = false;
        let mut open_publish = false;
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        // Dim the background and swallow clicks outside the dialog.
        egui::Area::new(egui::Id::new("modal_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                let rect = ctx.content_rect();
                ui.allocate_rect(rect, egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(120));
            });
        let title = match &modal {
            Modal::Discard(_) => "Discard changes",
            Modal::NewBranch { .. } => "New branch",
            Modal::DeleteBranch(_) => "Delete branch",
            Modal::DropStash(_) => "Drop stash",
            Modal::BranchPicker { .. } => "Switch branch",
            Modal::CheckoutConfirm { .. } => "Uncommitted changes",
            Modal::PublishGithub { .. } => "Publish to GitHub",
            Modal::Confirm { title, .. } => title,
            Modal::Input { kind, .. } => kind.title(),
            Modal::Reset { .. } => "Reset current branch",
            Modal::StashOpts { .. } => "Stash changes",
            Modal::StateMenu => "Operation in progress",
            Modal::Help => "Keyboard shortcuts",
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // Never wider than the pane: a narrow split would push the
                // dialog off the left edge.
                let want: f32 = match modal {
                    Modal::BranchPicker { .. } | Modal::Help => 460.0,
                    _ => 360.0,
                };
                let room = (ctx.content_rect().width() - 48.0).max(120.0);
                ui.set_min_width(want.min(room));
                match modal {
                    Modal::Discard(paths) => {
                        ui.label(format!(
                            "Throw away working tree changes in {} file{}? This cannot be undone.",
                            paths.len(),
                            if paths.len() == 1 { "" } else { "s" }
                        ));
                        for p in paths.iter().take(8) {
                            ui.monospace(p);
                        }
                        if paths.len() > 8 {
                            ui.weak(format!("and {} more", paths.len() - 8));
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Discard").clicked() || enter {
                                cmd = Some(Command::Discard(paths.clone()));
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                    }
                    Modal::NewBranch {
                        mut name,
                        from,
                        from_label,
                        mut checkout,
                    } => {
                        ui.label(format!("From {from_label}"));
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut name)
                                .hint_text("branch name")
                                .desired_width(f32::INFINITY),
                        );
                        if !resp.has_focus() && !ctx.egui_wants_keyboard_input() {
                            resp.request_focus();
                        }
                        ui.checkbox(&mut checkout, "check out after creating");
                        let valid = !name.trim().is_empty() && !name.contains(' ');
                        ui.horizontal(|ui| {
                            if ui.add_enabled(valid, egui::Button::new("Create")).clicked()
                                || (enter && valid)
                            {
                                cmd = Some(Command::CreateBranch {
                                    name: name.trim().to_owned(),
                                    from,
                                    checkout,
                                });
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if !close {
                            self.modal = Some(Modal::NewBranch {
                                name,
                                from,
                                from_label,
                                checkout,
                            });
                        }
                    }
                    Modal::DeleteBranch(name) => {
                        ui.label(format!("Delete local branch {name}?"));
                        ui.horizontal(|ui| {
                            if ui.button("Delete").clicked() || enter {
                                cmd = Some(Command::DeleteBranch(name.clone()));
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                    }
                    Modal::DropStash(i) => {
                        ui.label(format!("Drop stash {i}? This cannot be undone."));
                        ui.horizontal(|ui| {
                            if ui.button("Drop").clicked() || enter {
                                cmd = Some(Command::StashDrop(i));
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                    }
                    Modal::BranchPicker { mut filter } => {
                        if !close {
                            let busy = self.busy > 0;
                            let snapshot = self.snapshot.clone();
                            let theme = self.theme.clone();
                            match branch_picker::show(ui, &snapshot, &theme, &mut filter, busy)
                            {
                                Some(branch_picker::BranchPickerAction::Select(name)) => {
                                    switch_branch = Some(name);
                                    close = true;
                                }
                                Some(branch_picker::BranchPickerAction::CreateNew) => {
                                    open_new_branch = true;
                                    close = true;
                                }
                                Some(branch_picker::BranchPickerAction::PublishGithub) => {
                                    open_publish = true;
                                    close = true;
                                }
                                None => {}
                            }
                        }
                        if esc {
                            close = true;
                        }
                        if !close {
                            self.modal = Some(Modal::BranchPicker { filter });
                        }
                    }
                    Modal::CheckoutConfirm { target } => {
                        let s = &self.snapshot;
                        ui.label(format!("Switch to `{target}`?"));
                        ui.add_space(4.0);
                        ui.label("Your local changes would be overwritten or must be moved first:");
                        ui.weak(format!(
                            "{} unstaged, {} staged, {} conflicted",
                            s.unstaged.len(),
                            s.staged.len(),
                            s.conflicted.len()
                        ));
                        ui.add_space(4.0);
                        let stash_msg = self.stash_message_for_switch(&target);
                        ui.horizontal(|ui| {
                            if ui.button("Stash and switch").clicked() || enter {
                                cmd = Some(Command::StashAndCheckout {
                                    branch: target.clone(),
                                    message: stash_msg,
                                });
                                close = true;
                            }
                            if ui.button("Discard and switch").clicked() {
                                cmd = Some(Command::ForceCheckout(target.clone()));
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                    }
                    Modal::PublishGithub {
                        mut name,
                        mut description,
                        mut private,
                    } => {
                        ui.label("Create a GitHub repository and push the current branch.");
                        ui.weak("Uses GitHub CLI (gh). Run gh auth login first.");
                        ui.add_space(4.0);
                        ui.label("Repository name");
                        let name_resp = ui.add(
                            egui::TextEdit::singleline(&mut name)
                                .hint_text("my-repo or owner/my-repo")
                                .desired_width(f32::INFINITY),
                        );
                        if !name_resp.has_focus() && !ctx.egui_wants_keyboard_input() {
                            name_resp.request_focus();
                        }
                        ui.label("Description (optional)");
                        ui.add(
                            egui::TextEdit::singleline(&mut description)
                                .hint_text("Short description")
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut private, false, "Public");
                            ui.radio_value(&mut private, true, "Private");
                        });
                        let valid = Self::valid_github_repo_name(&name);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(valid, egui::Button::new("Create and push"))
                                .clicked()
                                || (enter && valid)
                            {
                                cmd = Some(Command::PublishGithub {
                                    name: name.trim().to_owned(),
                                    description: description.trim().to_owned(),
                                    private,
                                });
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if !close {
                            self.modal = Some(Modal::PublishGithub {
                                name,
                                description,
                                private,
                            });
                        }
                    }
                    Modal::Confirm {
                        body,
                        button,
                        cmd: action,
                        ..
                    } => {
                        ui.add(egui::Label::new(body).wrap());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button(button).clicked() || enter {
                                cmd = Some(action.clone());
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                    }
                    Modal::Input {
                        kind,
                        mut value,
                        mut extra,
                    } => {
                        let (hint, hint2) = kind.hints();
                        if let InputKind::Tag { label, .. } = &kind {
                            ui.label(format!("At {label}"));
                        }
                        let edit = if kind.multiline() {
                            egui::TextEdit::multiline(&mut value)
                                .hint_text(hint)
                                .desired_rows(4)
                                .desired_width(f32::INFINITY)
                        } else {
                            egui::TextEdit::singleline(&mut value)
                                .hint_text(hint)
                                .desired_width(f32::INFINITY)
                        };
                        let resp = ui.add(edit);
                        if !resp.has_focus() && !ctx.egui_wants_keyboard_input() {
                            resp.request_focus();
                        }
                        if let Some(h2) = hint2 {
                            ui.add(
                                egui::TextEdit::singleline(&mut extra)
                                    .hint_text(h2)
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        if matches!(kind, InputKind::Reword { .. }) {
                            ui.weak("Rewrites history from this commit up to HEAD.");
                        }
                        let valid = kind.valid(&value, &extra);
                        // Enter confirms single-line inputs; multiline needs Ctrl+Enter.
                        let confirm_key = if kind.multiline() {
                            ctx.input(|i| ctrl(i, egui::Key::Enter))
                        } else {
                            enter
                        };
                        ui.horizontal(|ui| {
                            if ui.add_enabled(valid, egui::Button::new("OK")).clicked()
                                || (confirm_key && valid)
                            {
                                cmd = Some(kind.command(&value, &extra));
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if !close {
                            self.modal = Some(Modal::Input { kind, value, extra });
                        }
                    }
                    Modal::Reset { oid, label } => {
                        ui.label(format!("Move the current branch to {label}"));
                        ui.add_space(4.0);
                        let mut pick = |kind: ResetKind, text: &str, tip: &str| {
                            if ui.button(text).on_hover_text(tip).clicked() {
                                cmd = Some(Command::Reset { oid, kind });
                                close = true;
                            }
                        };
                        pick(
                            ResetKind::Soft,
                            "Soft: keep changes staged",
                            "Index and working tree stay as they are",
                        );
                        pick(
                            ResetKind::Mixed,
                            "Mixed: keep changes unstaged",
                            "Index is reset, the working tree stays",
                        );
                        pick(
                            ResetKind::Hard,
                            "Hard: discard changes",
                            "Index and working tree are reset. Cannot be undone.",
                        );
                        if esc {
                            close = true;
                        }
                    }
                    Modal::StashOpts {
                        mut message,
                        mut keep_index,
                        mut include_untracked,
                    } => {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut message)
                                .hint_text("stash message (optional)")
                                .desired_width(f32::INFINITY),
                        );
                        if !resp.has_focus() && !ctx.egui_wants_keyboard_input() {
                            resp.request_focus();
                        }
                        ui.checkbox(&mut keep_index, "keep staged changes in the index");
                        ui.checkbox(&mut include_untracked, "include untracked files");
                        ui.horizontal(|ui| {
                            if ui.button("Stash").clicked() || enter {
                                cmd = Some(Command::StashPushOpts {
                                    message: message.clone(),
                                    keep_index,
                                    include_untracked,
                                });
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if !close {
                            self.modal = Some(Modal::StashOpts {
                                message,
                                keep_index,
                                include_untracked,
                            });
                        }
                    }
                    Modal::StateMenu => {
                        let s = &self.snapshot;
                        let what = s.state.label();
                        let progress = s
                            .rebase_progress
                            .map(|(d, t)| format!(" ({d} of {t})"))
                            .unwrap_or_default();
                        ui.label(format!("A {what} is in progress{progress}."));
                        if !s.conflicted.is_empty() {
                            ui.colored_label(
                                self.theme.error,
                                format!("{} conflicted file(s) to resolve", s.conflicted.len()),
                            );
                        }
                        ui.add_space(4.0);
                        let mut action = None;
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(s.conflicted.is_empty(), egui::Button::new("Continue"))
                                .clicked()
                            {
                                action = Some(StateAction::Continue);
                            }
                            if s.state != RepoState::Merge && ui.button("Skip this commit").clicked() {
                                action = Some(StateAction::Skip);
                            }
                            if ui.button("Abort").clicked() {
                                action = Some(StateAction::Abort);
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if let Some(a) = action {
                            close = true;
                            self.modal = None;
                            self.state_action(a);
                            if self.modal.is_some() {
                                // state_action opened a confirmation; keep it.
                                return;
                            }
                        }
                    }
                    Modal::Help => {
                        help::show(ui);
                        if esc {
                            close = true;
                        }
                    }
                }
                if !close {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("Close").on_hover_text("Escape").clicked() {
                                    close = true;
                                }
                            },
                        );
                    });
                }
            });
        if let Some(c) = cmd {
            self.run(c);
        }
        if let Some(target) = switch_branch {
            self.try_switch_branch(target);
            return;
        }
        if open_new_branch {
            let s = &self.snapshot;
            let from = s
                .head
                .as_ref()
                .and_then(|h| h.oid)
                .or_else(|| s.commits.first().map(|c| c.oid))
                .unwrap_or(git2::Oid::ZERO_SHA1.to_owned());
            let from_label = s
                .head
                .as_ref()
                .and_then(|h| h.branch_name.clone())
                .unwrap_or_else(|| "HEAD".into());
            self.modal = Some(Modal::NewBranch {
                name: String::new(),
                from,
                from_label,
                checkout: true,
            });
            return;
        }
        if open_publish {
            self.open_publish_github();
            return;
        }
        if close {
            self.modal = None;
            ctx.memory_mut(|m| m.request_focus(egui::Id::NULL));
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let open_picker = std::cell::Cell::new(false);
        let state_action: std::cell::Cell<Option<StateAction>> = std::cell::Cell::new(None);
        let open_state = std::cell::Cell::new(false);
        let error_color = self.theme.error;
        let snapshot = self.snapshot.clone();
        let busy = self.busy;
        let modal_open = self.modal.is_some();
        let show_debug = self.show_debug;
        let debug = format!("{:.1} ms {} x{}", self.frame_ms, self.transport, self.scale);
        let last_op = self.last_op.clone();
        row::split(
            ui,
            |ui| {
                toolbar::show(self, ui);
                if show_debug {
                    ui.separator();
                    ui.weak(debug);
                }
                ui.separator();
            },
            |ui| {
                let s = &snapshot;
                let can_pick_branch = busy == 0 && !modal_open;
                let counts = format!("{} unstaged, {} staged", s.unstaged.len(), s.staged.len());
                let (name, ahead_behind) = match &s.head {
                    Some(h) => {
                        let name = h.branch_name.clone().unwrap_or_else(|| {
                            h.oid
                                .map(|o| format!("detached {}", crate::git::repo::short_id(o)))
                                .unwrap_or_else(|| "no HEAD".into())
                        });
                        let ab = s
                            .branches
                            .iter()
                            .find(|b| b.is_head)
                            .filter(|b| b.ahead > 0 || b.behind > 0)
                            .map(|b| match (b.ahead, b.behind) {
                                // The bundled fonts have no arrow glyphs.
                                (a, 0) => format!("{a} ahead"),
                                (0, b) => format!("{b} behind"),
                                (a, b) => format!("{a} ahead, {b} behind"),
                            });
                        (Some(name), ab)
                    }
                    None => (None, None),
                };
                // The path is the least important item: give it only what the
                // branch and the counts leave over, so those two stay visible.
                let font = egui::TextStyle::Body.resolve(ui.style());
                let measure = |t: &str| {
                    ui.painter()
                        .layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::WHITE)
                        .size()
                        .x
                };
                let spacing = ui.spacing().item_spacing.x;
                let mut reserved = measure(name.as_deref().unwrap_or("loading")) + 16.0 + spacing * 4.0 + 8.0;
                reserved += measure(&counts) + spacing * 2.0 + 8.0;
                if let Some(ab) = &ahead_behind {
                    reserved += measure(ab) + spacing;
                }
                if let Some(op) = &last_op {
                    reserved += measure(op) + spacing * 2.0 + 8.0;
                }
                let path = s.path.to_string_lossy();
                let shown = match std::env::var("HOME") {
                    Ok(h) if path.starts_with(&h) => format!("~{}", &path[h.len()..]),
                    _ => path.to_string(),
                };
                let shown = shown.trim_end_matches('/').to_owned();
                let in_progress = s.state != crate::git::repo::RepoState::Clean;
                let state_text = if in_progress {
                    let progress = s
                        .rebase_progress
                        .map(|(d, t)| format!(" {d}/{t}"))
                        .unwrap_or_default();
                    format!("{}{progress}", s.state.label().to_uppercase())
                } else {
                    String::new()
                };
                if in_progress {
                    reserved += measure(&state_text) + 190.0 + spacing * 5.0;
                }
                let path_w = (ui.available_width() - reserved).min(measure(&shown) + 2.0);
                if path_w > 24.0 && !in_progress {
                    ui.add_sized(
                        [path_w, ui.spacing().interact_size.y],
                        egui::Label::new(shown).truncate(),
                    );
                    ui.separator();
                }
                if in_progress {
                    let can_continue = s.conflicted.is_empty() && busy == 0 && !modal_open;
                    if ui
                        .add(
                            egui::Label::new(egui::RichText::new(&state_text).strong().color(error_color))
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text("m: continue, abort or skip")
                        .clicked()
                    {
                        open_state.set(true);
                    }
                    if ui
                        .add_enabled(can_continue, egui::Button::new("Continue").small())
                        .on_hover_text(if s.conflicted.is_empty() {
                            "git --continue".to_owned()
                        } else {
                            format!("{} conflicted file(s) left", s.conflicted.len())
                        })
                        .clicked()
                    {
                        state_action.set(Some(StateAction::Continue));
                    }
                    if ui
                        .add_enabled(busy == 0 && !modal_open, egui::Button::new("Abort").small())
                        .clicked()
                    {
                        state_action.set(Some(StateAction::Abort));
                    }
                    ui.separator();
                }
                match name {
                    Some(name) => {
                        let branch_color = ui.visuals().text_color();
                        let mut picked = false;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            if ui
                                .add(
                                    egui::Label::new(egui::RichText::new(name).strong())
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text("Switch branch")
                                .clicked()
                            {
                                picked = true;
                            }
                            if icons::chevron_down(ui, branch_color)
                                .on_hover_text("Switch branch")
                                .clicked()
                            {
                                picked = true;
                            }
                        });
                        if picked && can_pick_branch {
                            open_picker.set(true);
                        }
                        if let Some(ab) = ahead_behind {
                            ui.weak(ab);
                        }
                    }
                    None => {
                        ui.weak("loading");
                    }
                }
                ui.separator();
                ui.weak(counts);
                if let Some(op) = last_op {
                    ui.separator();
                    ui.weak(op);
                }
            },
        );
        if open_picker.get() {
            self.open_branch_picker();
        }
        if open_state.get() {
            self.open_state_menu();
        }
        if let Some(a) = state_action.get() {
            self.state_action(a);
        }
    }

    fn show_toasts(&self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for t in &self.toasts {
                    let color = if t.error {
                        self.theme.error
                    } else {
                        self.theme.ok
                    };
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.0, color))
                        .show(ui, |ui| {
                            ui.set_max_width(420.0);
                            ui.colored_label(color, &t.text);
                        });
                }
            });
    }
}

/// Modifiers of the first press of `key` in this frame's events, if any.
/// Terminals deliver modifiers per key event, so this is more reliable
/// than the global modifier state.
fn key_press(i: &egui::InputState, key: egui::Key) -> Option<egui::Modifiers> {
    i.events.iter().find_map(|e| match e {
        egui::Event::Key {
            key: k,
            pressed: true,
            modifiers,
            ..
        } if *k == key => Some(*modifiers),
        _ => None,
    })
}

fn plain(i: &egui::InputState, key: egui::Key) -> bool {
    key_press(i, key).is_some_and(|m| !m.ctrl && !m.shift && !m.alt)
}

fn shifted(i: &egui::InputState, key: egui::Key) -> bool {
    key_press(i, key).is_some_and(|m| m.shift && !m.ctrl && !m.alt)
}

fn ctrl(i: &egui::InputState, key: egui::Key) -> bool {
    key_press(i, key).is_some_and(|m| m.ctrl)
}

/// Human readable age like "3m", "2h", "5d", "3mo", "2y".
pub fn age(now: i64, then: i64) -> String {
    let d = (now - then).max(0);
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else if d < 86400 * 30 {
        format!("{}d", d / 86400)
    } else if d < 86400 * 365 {
        format!("{}mo", d / (86400 * 30))
    } else {
        format!("{}y", d / (86400 * 365))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_buckets() {
        assert_eq!(age(1000, 990), "10s");
        assert_eq!(age(1000, 1000 - 120), "2m");
        assert_eq!(age(1000, 1000 - 7200), "2h");
        assert_eq!(age(1_000_000, 1_000_000 - 86400 * 3), "3d");
        assert_eq!(age(100_000_000, 100_000_000 - 86400 * 45), "1mo");
        assert_eq!(age(100_000_000, 100_000_000 - 86400 * 800), "2y");
    }
}
