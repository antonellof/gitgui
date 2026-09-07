//! Top-level state, messages and update logic. Views live in the sibling
//! modules; `App::view` assembles them on an iced `pane_grid` so every pane
//! can be dragged, resized and maximized.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use git2::Oid;
use iced_core::keyboard::{self, key::Named};
use iced_core::widget::Operation;
use iced_core::{Length, Point};
use iced_widget::pane_grid::{self, Axis, Configuration};
use iced_widget::{column, container, pane_grid as pane_grid_widget, stack, text, text_editor};

use crate::git::actions::{ConflictSide, ResetKind};
use crate::git::ops::{Command, Reply, StateAction};
use crate::git::rebase::TodoAction;
use crate::git::repo::{DiffOpts, DiffTarget, DirEntry, FileStatus, RepoSnapshot, RepoState};
use crate::ui::editor::Editor;
use crate::ui::theme::Theme;
use crate::ui::merge::{MergeState, Resolution};
use crate::ui::{changes, diff, editor, footer, log, menu, merge, modal, sidebar, widgets};

pub type Renderer = crate::shell::Renderer;
pub type Element<'a> = iced_core::Element<'a, Message, iced_core::Theme, Renderer>;

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
    Changes,
    Detail,
}

impl Pane {
    pub fn title(self) -> &'static str {
        match self {
            Pane::Sidebar => "Repository",
            Pane::Log => "Commits",
            Pane::Changes => "Changes",
            Pane::Detail => "Diff",
        }
    }
}

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
    BranchPicker {
        filter: String,
    },
    CheckoutConfirm {
        target: String,
    },
    PublishGithub {
        name: String,
        description: String,
        private: bool,
    },
    Confirm {
        title: &'static str,
        body: String,
        button: &'static str,
        cmd: Command,
    },
    Input {
        kind: InputKind,
        value: String,
        extra: String,
    },
    Reset {
        oid: Oid,
        label: String,
    },
    StashOpts {
        message: String,
        keep_index: bool,
        include_untracked: bool,
    },
    StateMenu,
    Help,
    CloseEditor,
}

/// What an [`Modal::Input`] dialog is for. `value` is the first field,
/// `extra` the optional second one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    Tag { oid: Oid, label: String },
    RenameBranch { old: String },
    Reword { oid: Oid, is_root: bool },
    RemoteAdd,
    RemoteUrl { name: String },
    RemoteRename { old: String },
    SetUpstream { branch: String },
    BranchFromStash { index: usize },
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

    pub fn valid(&self, value: &str, extra: &str) -> bool {
        let v = value.trim();
        match self {
            InputKind::RemoteAdd => !v.is_empty() && !v.contains(' ') && !extra.trim().is_empty(),
            InputKind::Reword { .. } | InputKind::Ignore | InputKind::RemoteUrl { .. } => !v.is_empty(),
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

/// Whether a commit can be rewritten with a rebase from the current branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewriteInfo {
    pub is_root: bool,
    pub is_head: bool,
    pub has_older: bool,
}

pub struct NetLog {
    pub label: &'static str,
    pub lines: Vec<String>,
    pub running: bool,
    pub open: bool,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// A right-click menu: where it opened and for what.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub at: Point,
    pub kind: MenuKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuKind {
    Commit(usize),
    Branch(String),
    RemoteBranch(String),
    Remote(String),
    Tag(String),
    Stash(usize),
    /// (path, staged, conflicted, untracked)
    File {
        path: String,
        staged: bool,
        conflicted: bool,
        untracked: bool,
    },
    TreeFile(String),
    TreeDir(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitAction {
    NewBranch,
    Tag,
    CherryPick,
    Revert,
    Reset,
    CheckoutDetached,
    Rewrite(TodoAction),
    Reword,
    Autosquash,
    CreateFixup,
    CopyHash,
    CopyMessage,
    OpenBrowser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkAction {
    Stage,
    Unstage,
    Discard,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A key press no widget consumed.
    Key(keyboard::Key, keyboard::Modifiers),
    Nothing,
    // Panes
    PaneClicked(pane_grid::Pane),
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    PaneMaximize(pane_grid::Pane),
    PaneRestore,
    // Selection
    SelectRow(Selection),
    SelectFile(DiffTarget),
    SidebarSelect(String, Oid),
    FilterChanged(String),
    FilterClear,
    FilterOpen,
    // Git
    Run(Command),
    Refresh,
    Switch(String),
    CheckoutDetached(Oid),
    Discard(Vec<String>),
    DiscardAll,
    Ignore(String),
    Resolve(String, ConflictSide),
    Commit,
    CommitAndPush,
    CommitMsg(text_editor::Action),
    ToggleAmend(bool),
    CommitAction(usize, CommitAction),
    StateAction(StateAction),
    // Diff
    DiffSearch(String),
    DiffSearchOpen,
    DiffSearchClose,
    DiffNext(i32),
    DiffContext(i32),
    DiffWhitespace,
    DiffWrap,
    EditorWrap,
    DiffLineClick { hunk: usize, line: usize, shift: bool },
    DiffDragTo { hunk: usize, line: usize },
    DiffHunk(HunkAction, usize),
    LinesStage,
    LinesUnstage,
    LinesDiscard,
    ClearLineSel,
    // Dialogs
    ModalClose,
    ModalConfirm,
    ModalValue(String),
    ModalExtra(String),
    ModalMultiline(text_editor::Action),
    ModalCheckbox(bool),
    ModalCheckbox2(bool),
    ModalReset(ResetKind),
    ModalCheckoutStash,
    ModalCheckoutForce,
    ModalPick(String),
    ModalEditorSave,
    ModalEditorDiscard,
    OpenBranchPicker,
    OpenPublish,
    OpenHelp,
    OpenStateMenu,
    OpenStashDialog,
    OpenNewBranch,
    // Menus
    MenuOpen(MenuKind),
    MenuClose,
    MenuPick(Box<Message>),
    Input(InputKind, String, String),
    Confirm(&'static str, String, &'static str, Command),
    Modal(Modal),
    Copy(String),
    PullRequest(String),
    // Tree
    TreeToggle(String),
    TreeOpen(String),
    TreeRequest(String),
    ShowChanges(String),
    // Merge tool
    MergeOpen(String),
    MergeSet(usize, Option<Resolution>),
    MergeAll(Resolution),
    MergeApply,
    MergeEdit,
    MergeClose,
    // Editor
    Edit,
    EditorAction(text_editor::Action),
    EditorSave,
    EditorClose,
    EditorExternal,
    EditorPreview,
    // Misc
    SectionToggle(&'static str),
    NetClose,
    Quit,
    InitRepo,
}

pub struct App {
    pub theme: Theme,
    pub snapshot: Arc<RepoSnapshot>,
    pub have_snapshot: bool,
    pub selection: Selection,
    pub focus: Pane,
    pub commit_files: HashMap<Oid, Vec<FileStatus>>,
    pub selected_file: Option<DiffTarget>,
    pub diff: Option<crate::git::repo::DiffText>,
    pub diff_loading: bool,
    pub filter: String,
    pub filter_active: bool,
    pub filtered: Vec<usize>,
    pub commit_msg: text_editor::Content<Renderer>,
    pub amend: bool,
    pub toasts: Vec<Toast>,
    pub last_op: Option<String>,
    pub pending: Vec<Command>,
    /// Widget operations (focus requests) for the shell to run.
    pub ops: Vec<Box<dyn Operation>>,
    pub scroll_to_selection: Cell<bool>,
    pub frame_ms: f32,
    pub transport: &'static str,
    pub scale: f32,
    pub show_debug: bool,
    pub sidebar_selected: Option<String>,
    /// Collapsed sidebar sections by title.
    pub sidebar_collapsed: HashSet<&'static str>,
    pub modal: Option<Modal>,
    pub modal_multiline: text_editor::Content<Renderer>,
    pub menu: Option<Menu>,
    pub cursor: Point,
    /// Logical window size, set by the shell before each frame.
    pub window: iced_core::Size,
    /// Modifier keys as of the last input event (for shift-click in the diff).
    pub modifiers: keyboard::Modifiers,
    /// Text to put on the terminal clipboard after this frame.
    pub pending_copy: Vec<String>,
    pub net: NetLog,
    pub amend_loaded: bool,
    pub busy: usize,
    pub quit: bool,
    pub no_repo: bool,
    pub repo_path: PathBuf,
    pub diff_opts: DiffOpts,
    pub diff_search: String,
    pub diff_search_active: bool,
    /// Wrap long lines in the diff viewer / the editor.
    pub wrap: bool,
    pub editor_wrap: bool,
    pub diff_match: usize,
    pub diff_jump: Cell<bool>,
    pub editor: Option<Editor>,
    /// Three-way conflict resolver, shown in the diff pane while open.
    pub merge: Option<MergeState>,
    pub editor_cmd: Option<String>,
    pub open_on_start: Option<String>,
    pub tree: HashMap<String, Vec<DirEntry>>,
    pub tree_open: HashSet<String>,
    pub tree_selected: Option<String>,
    pub tree_requested: HashSet<String>,
    pub line_sel: Option<LineSel>,
    pub panes: pane_grid::State<Pane>,
    /// Where the pane grid was drawn last frame (for title bar hover).
    pub grid_bounds: Cell<iced_core::Rectangle>,
    /// Layout while a file opened from the tree is shown: sidebar + editor.
    pub editor_panes: pane_grid::State<Pane>,
    /// The editor takes the whole main area (opened from the tree or --open).
    pub editor_full: bool,
}

impl App {
    pub fn new(theme: Theme, transport: &'static str, scale: f32, repo_path: PathBuf) -> Self {
        let panes = pane_grid::State::with_configuration(Configuration::Split {
            axis: Axis::Vertical,
            ratio: 0.2,
            a: Box::new(Configuration::Pane(Pane::Sidebar)),
            b: Box::new(Configuration::Split {
                axis: Axis::Horizontal,
                ratio: 0.5,
                a: Box::new(Configuration::Pane(Pane::Log)),
                b: Box::new(Configuration::Split {
                    axis: Axis::Vertical,
                    ratio: 0.36,
                    a: Box::new(Configuration::Pane(Pane::Changes)),
                    b: Box::new(Configuration::Pane(Pane::Detail)),
                }),
            }),
        });
        let editor_panes = pane_grid::State::with_configuration(Configuration::Split {
            axis: Axis::Vertical,
            ratio: 0.2,
            a: Box::new(Configuration::Pane(Pane::Sidebar)),
            b: Box::new(Configuration::Pane(Pane::Detail)),
        });
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
            filtered: Vec::new(),
            commit_msg: text_editor::Content::new(),
            amend: false,
            toasts: Vec::new(),
            last_op: None,
            pending: Vec::new(),
            ops: Vec::new(),
            scroll_to_selection: Cell::new(false),
            frame_ms: 0.0,
            transport,
            scale,
            show_debug: false,
            sidebar_selected: None,
            sidebar_collapsed: HashSet::new(),
            modal: None,
            modal_multiline: text_editor::Content::new(),
            menu: None,
            cursor: Point::ORIGIN,
            window: iced_core::Size::new(800.0, 500.0),
            modifiers: keyboard::Modifiers::empty(),
            pending_copy: Vec::new(),
            net: NetLog {
                label: "",
                lines: Vec::new(),
                running: false,
                open: false,
            },
            amend_loaded: false,
            busy: 0,
            quit: false,
            no_repo: false,
            repo_path,
            diff_opts: DiffOpts::default(),
            diff_search: String::new(),
            diff_search_active: false,
            wrap: false,
            editor_wrap: false,
            diff_match: 0,
            diff_jump: Cell::new(false),
            editor: None,
            merge: None,
            editor_cmd: None,
            open_on_start: None,
            tree: HashMap::new(),
            tree_open: HashSet::new(),
            tree_selected: None,
            tree_requested: HashSet::new(),
            line_sel: None,
            panes,
            grid_bounds: Cell::new(iced_core::Rectangle::default()),
            editor_panes,
            editor_full: false,
        }
    }

    fn in_editor_layout(&self) -> bool {
        (self.editor.is_some() && self.editor_full) || self.merge.is_some()
    }

    fn active_panes(&self) -> &pane_grid::State<Pane> {
        if self.in_editor_layout() {
            &self.editor_panes
        } else {
            &self.panes
        }
    }

    /// Title bar strips of the active layout, in window coordinates.
    pub fn title_strips(&self) -> Vec<(pane_grid::Pane, iced_core::Rectangle)> {
        let grid = self.grid_bounds.get();
        if grid.width <= 0.0 {
            return Vec::new();
        }
        self.active_panes()
            .layout()
            .pane_regions(widgets::PANE_SPACING, widgets::PANE_MIN, grid.size())
            .into_iter()
            .map(|(pane, r)| {
                (
                    pane,
                    iced_core::Rectangle::new(
                        Point::new(grid.x + r.x, grid.y + r.y),
                        iced_core::Size::new(r.width, widgets::TITLE_H),
                    ),
                )
            })
            .collect()
    }

    /// The pane whose title bar is under the pointer: the drag handle.
    pub fn hovered_pane(&self) -> Option<pane_grid::Pane> {
        if let Some(max) = self.active_panes().maximized() {
            return self.title_strips().iter().find(|(p, r)| *p == max && r.contains(self.cursor)).map(|(p, _)| *p);
        }
        self.title_strips().iter().find(|(_, r)| r.contains(self.cursor)).map(|(p, _)| *p)
    }

    fn active_panes_mut(&mut self) -> &mut pane_grid::State<Pane> {
        if self.in_editor_layout() {
            &mut self.editor_panes
        } else {
            &mut self.panes
        }
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

    /// Toasts still on screen (drops expired ones).
    pub fn toasts_active(&mut self) -> bool {
        self.toasts
            .retain(|t| t.at.elapsed().as_secs_f32() < if t.error { 8.0 } else { 3.0 });
        !self.toasts.is_empty()
    }

    pub fn has_worktree_row(&self) -> bool {
        self.snapshot.is_dirty() || self.snapshot.commits.is_empty()
    }

    pub fn commit_message_text(&self) -> String {
        self.commit_msg.text()
    }

    fn set_commit_message(&mut self, text: &str) {
        self.commit_msg = text_editor::Content::with_text(text);
    }

    pub fn pane_of(&self, kind: Pane) -> Option<pane_grid::Pane> {
        self.active_panes().iter().find(|(_, k)| **k == kind).map(|(p, _)| *p)
    }

    // ---- replies from the worker ----

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
                self.refresh_tree();
                if let Some(m) = &self.merge {
                    if !self.snapshot.conflicted.iter().any(|f| f.path == m.path) {
                        self.merge = None;
                    }
                }
                if first {
                    self.selection = if self.has_worktree_row() || self.snapshot.commits.is_empty() {
                        Selection::WorkingTree
                    } else {
                        Selection::Commit(0)
                    };
                    self.on_selection_changed();
                    if let Some(path) = self.open_on_start.take() {
                        self.tree_selected = Some(path.clone());
                        self.open_editor(path, true);
                    }
                } else {
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
            Reply::DirEntries(dir, result) => {
                self.tree_requested.remove(&dir);
                match result {
                    Ok(entries) => {
                        self.tree.insert(dir, entries);
                    }
                    Err(e) => {
                        self.tree_open.remove(&dir);
                        self.toast(format!("cannot list {dir}: {e}"), true);
                    }
                }
            }
            Reply::Op { label, result } => {
                self.busy = self.busy.saturating_sub(1);
                match result {
                    Ok(msg) => {
                        if label == "commit" {
                            self.set_commit_message("");
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

    // ---- selection ----

    pub fn selected_worktree_file(&self) -> Option<(String, bool)> {
        match &self.selected_file {
            Some(DiffTarget::WorkdirUnstaged(p)) => Some((p.clone(), false)),
            Some(DiffTarget::Staged(p)) => Some((p.clone(), true)),
            _ => None,
        }
    }

    pub fn selected_commit(&self) -> Option<usize> {
        match self.selection {
            Selection::Commit(i) if i < self.snapshot.commits.len() => Some(i),
            _ => None,
        }
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
        let rows = self.log_rows();
        if !rows.contains(&self.selection) {
            if let Some(first) = rows.first().copied() {
                self.select(first);
                self.scroll_to_selection.set(true);
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
                        let first = files.first().map(|f| DiffTarget::Commit(oid, f.path.clone()));
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
        // Conflicts first: they block everything else.
        let first = s
            .conflicted
            .first()
            .map(|f| DiffTarget::WorkdirUnstaged(f.path.clone()))
            .or_else(|| s.unstaged.first().map(|f| DiffTarget::WorkdirUnstaged(f.path.clone())))
            .or_else(|| s.staged.first().map(|f| DiffTarget::Staged(f.path.clone())));
        self.select_file(first);
    }

    pub fn select_file(&mut self, target: Option<DiffTarget>) {
        if self.selected_file != target {
            self.line_sel = None;
            self.follow_selection_in_editor(target.as_ref());
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

    /// A clean editor switches to the newly selected file; a dirty one stays.
    fn follow_selection_in_editor(&mut self, target: Option<&DiffTarget>) {
        let Some(ed) = &self.editor else { return };
        if ed.dirty() {
            return;
        }
        let Some(t) = target else {
            self.editor = None;
            return;
        };
        if ed.path == t.path() {
            return;
        }
        let workdir = self.snapshot.path.clone();
        self.editor = Editor::open(&workdir, t.path()).ok();
    }

    pub fn log_rows(&self) -> Vec<Selection> {
        let mut rows = Vec::with_capacity(self.filtered.len() + 1);
        if self.has_worktree_row() && self.filter.trim().is_empty() {
            rows.push(Selection::WorkingTree);
        }
        rows.extend(self.filtered.iter().map(|i| Selection::Commit(*i)));
        rows
    }

    pub fn move_selection(&mut self, delta: i32) {
        let rows = self.log_rows();
        if rows.is_empty() {
            return;
        }
        let cur = rows.iter().position(|r| *r == self.selection).unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, rows.len() as i32 - 1) as usize;
        self.select(rows[next]);
        self.scroll_to_selection.set(true);
    }

    // ---- staging and commits ----

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

    pub fn toggle_stage_selected(&mut self) {
        match self.selected_worktree_file() {
            Some((p, false)) => self.run(Command::Stage(vec![p])),
            Some((p, true)) => self.run(Command::Unstage(vec![p])),
            None => {}
        }
    }

    fn commit_now(&mut self, push: bool) {
        let msg = self.commit_message_text().trim().to_owned();
        if msg.is_empty() {
            self.toast("commit message is empty", true);
            return;
        }
        if self.snapshot.staged.is_empty() && !self.amend {
            self.toast("nothing staged", true);
            return;
        }
        let amend = self.amend;
        self.run(if push {
            Command::CommitAndPush { message: msg, amend }
        } else {
            Command::Commit { message: msg, amend }
        });
    }

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
        if matches!(kind, InputKind::Reword { .. }) {
            self.modal_multiline = text_editor::Content::with_text(&value);
        }
        self.modal = Some(Modal::Input { kind, value, extra });
        self.focus_modal_input();
    }

    fn focus_modal_input(&mut self) {
        self.ops.push(Box::new(iced_core::widget::operation::focusable::focus(
            widgets::MODAL_INPUT_ID.clone(),
        )));
    }

    fn commit_label(&self, idx: usize) -> String {
        self.snapshot
            .commits
            .get(idx)
            .map(|c| format!("{} {}", c.short, c.summary))
            .unwrap_or_default()
    }

    fn commit_action(&mut self, idx: usize, action: CommitAction) {
        let Some(c) = self.snapshot.commits.get(idx).cloned() else { return };
        match action {
            CommitAction::NewBranch => {
                self.modal = Some(Modal::NewBranch {
                    name: String::new(),
                    from: c.oid,
                    from_label: c.short.clone(),
                    checkout: true,
                });
                self.focus_modal_input();
            }
            CommitAction::Tag => {
                let label = self.commit_label(idx);
                self.input(InputKind::Tag { oid: c.oid, label }, String::new(), String::new());
            }
            CommitAction::CherryPick => {
                let body = format!("Apply {} on top of HEAD as a new commit?", self.commit_label(idx));
                self.confirm("Cherry-pick", body, "Cherry-pick", Command::CherryPick(c.oid));
            }
            CommitAction::Revert => {
                let body = format!("Create a commit that undoes {}?", self.commit_label(idx));
                self.confirm("Revert", body, "Revert", Command::Revert(c.oid));
            }
            CommitAction::Reset => {
                if self.busy == 0 {
                    self.modal = Some(Modal::Reset {
                        oid: c.oid,
                        label: self.commit_label(idx),
                    });
                }
            }
            CommitAction::CheckoutDetached => {
                if self.snapshot.is_dirty() {
                    self.toast("commit or stash your changes before checking out a commit", true);
                } else {
                    self.run(Command::CheckoutDetached(c.oid));
                }
            }
            CommitAction::Rewrite(todo) => self.commit_rewrite(idx, todo),
            CommitAction::Reword => self.commit_reword(idx),
            CommitAction::Autosquash => {
                let Some(info) = self.rewrite_info(idx) else {
                    self.toast("only commits on the current branch can be autosquashed", true);
                    return;
                };
                if self.clean_for_rebase() {
                    self.run(Command::Autosquash {
                        oid: c.oid,
                        is_root: info.is_root,
                    });
                }
            }
            CommitAction::CreateFixup => {
                if self.snapshot.staged.is_empty() {
                    self.toast("stage the changes for the fixup first", true);
                    return;
                }
                self.run(Command::Commit {
                    message: format!("fixup! {}", c.summary),
                    amend: false,
                });
            }
            CommitAction::CopyHash => {
                self.copy(c.oid.to_string());
                self.toast(format!("copied {}", c.short), false);
            }
            CommitAction::CopyMessage => {
                let mut msg = c.summary.clone();
                if !c.body.is_empty() {
                    msg.push_str("\n\n");
                    msg.push_str(&c.body);
                }
                self.copy(msg);
                self.toast("copied commit message", false);
            }
            CommitAction::OpenBrowser => {
                let url = self
                    .web_remote()
                    .and_then(|r| crate::git::actions::commit_url(r, c.oid));
                match url {
                    Some(u) => self.open_url(&u),
                    None => self.toast("no web remote for this repository", true),
                }
            }
        }
    }

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
            TodoAction::Drop => self.confirm(
                "Drop commit",
                format!("Remove {label} from the branch? Later commits are replayed on top."),
                "Drop",
                cmd,
            ),
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
        let c = self.snapshot.commits[idx].clone();
        let mut msg = c.summary.clone();
        if !c.body.is_empty() {
            msg.push_str("\n\n");
            msg.push_str(&c.body);
        }
        if info.is_head {
            self.set_commit_message(&msg);
            self.amend = true;
            self.amend_loaded = true;
            self.selection = Selection::WorkingTree;
            self.focus_commit_box();
            return;
        }
        if !self.clean_for_rebase() {
            return;
        }
        self.input(
            InputKind::Reword {
                oid: c.oid,
                is_root: info.is_root,
            },
            msg,
            String::new(),
        );
    }

    fn focus_commit_box(&mut self) {
        self.ops.push(Box::new(iced_core::widget::operation::focusable::focus(
            widgets::COMMIT_BOX_ID.clone(),
        )));
    }

    pub fn copy(&mut self, text: String) {
        self.pending_copy.push(text);
    }

    pub fn web_remote(&self) -> Option<&str> {
        let s = &self.snapshot;
        s.remote_urls
            .iter()
            .find(|(n, _)| n == "origin")
            .or_else(|| s.remote_urls.first())
            .map(|(_, u)| u.as_str())
    }

    pub fn open_url(&mut self, url: &str) {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
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
        self.diff_jump.set(true);
    }

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

    pub fn has_line_selection(&self) -> bool {
        self.line_action_target().is_some()
    }

    /// (unstaged side?) when a line selection can be acted on.
    pub fn line_selection_side(&self) -> Option<bool> {
        self.line_action_target().map(|(_, unstaged, _, _)| unstaged)
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
                    format!(
                        "Throw away {n} changed line{} of {path}? This cannot be undone.",
                        if n == 1 { "" } else { "s" }
                    ),
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

    pub fn state_action(&mut self, action: StateAction) {
        let Some(sub) = self.snapshot.state.git_subcommand() else {
            self.toast("no operation in progress", true);
            return;
        };
        if action == StateAction::Continue && !self.snapshot.conflicted.is_empty() {
            self.toast(
                format!(
                    "{} conflicted file(s) left, resolve them first",
                    self.snapshot.conflicted.len()
                ),
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

    pub fn try_switch_branch(&mut self, target: String) {
        if self.busy > 0 {
            return;
        }
        let current = self.snapshot.head.as_ref().and_then(|h| h.branch_name.clone());
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

    pub fn has_origin(&self) -> bool {
        self.snapshot.remotes.iter().any(|r| r == "origin")
    }

    fn default_github_repo_name(&self) -> String {
        self.snapshot
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repository")
            .to_owned()
    }

    fn valid_github_repo_name(name: &str) -> bool {
        let n = name.trim();
        !n.is_empty()
            && !n.contains(' ')
            && n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/' || c == '.')
    }

    // ---- file tree ----

    pub fn request_dir(&mut self, dir: &str) {
        if self.tree_requested.insert(dir.to_owned()) {
            self.pending.push(Command::ListDir(dir.to_owned()));
        }
    }

    fn refresh_tree(&mut self) {
        self.tree_requested.clear();
        let mut dirs: Vec<String> = self.tree_open.iter().cloned().collect();
        dirs.push(String::new());
        for d in dirs {
            self.request_dir(&d);
        }
    }

    pub fn toggle_dir(&mut self, dir: &str) {
        if self.tree_open.remove(dir) {
            return;
        }
        self.tree_open.insert(dir.to_owned());
        if !self.tree.contains_key(dir) {
            self.request_dir(dir);
        }
    }

    /// The file `e`, `Shift+E` and `Shift+O` act on: the open editor's file,
    /// then the tree selection while the sidebar has focus, then the file
    /// selected in the change lists.
    pub fn current_file(&self) -> Option<String> {
        if let Some(ed) = &self.editor {
            return Some(ed.path.clone());
        }
        if self.focus == Pane::Sidebar {
            if let Some(p) = &self.tree_selected {
                return Some(p.clone());
            }
        }
        self.selected_file.as_ref().map(|t| t.path().to_owned())
    }

    // ---- editor ----

    pub fn open_editor(&mut self, path: String, full: bool) {
        if let Some(ed) = &self.editor {
            if ed.path == path {
                return;
            }
            if ed.dirty() {
                self.toast(format!("{} has unsaved changes: Ctrl+S or Escape first", ed.path), true);
                return;
            }
        }
        let workdir = self.snapshot.path.clone();
        match Editor::open(&workdir, &path) {
            Ok(ed) => {
                self.editor = Some(ed);
                self.editor_full = full;
                self.focus = Pane::Detail;
                self.ops.push(Box::new(iced_core::widget::operation::focusable::focus(
                    widgets::EDITOR_ID.clone(),
                )));
            }
            Err(e) => self.toast(e, true),
        }
    }

    /// Open the three-way resolver on a conflicted file.
    pub fn open_merge(&mut self, path: String) {
        let head = self
            .snapshot
            .head
            .as_ref()
            .and_then(|h| h.branch_name.clone())
            .unwrap_or_else(|| "HEAD".into());
        let workdir = self.snapshot.path.clone();
        match MergeState::open(&workdir, &path, &head) {
            Ok(m) => {
                self.merge = Some(m);
                self.editor = None;
                self.selection = Selection::WorkingTree;
                self.select_file(Some(DiffTarget::WorkdirUnstaged(path)));
                self.focus = Pane::Detail;
            }
            Err(e) => self.toast(e, true),
        }
    }

    pub fn save_editor(&mut self) {
        let Some(ed) = self.editor.as_mut() else { return };
        match ed.save() {
            Ok(()) => {
                let p = ed.path.clone();
                self.toast(format!("saved {p}"), false);
                self.pending.push(Command::Refresh);
            }
            Err(e) => self.toast(format!("cannot save: {e}"), true),
        }
    }

    pub fn close_editor(&mut self) {
        let Some(ed) = &self.editor else { return };
        if ed.dirty() {
            self.modal = Some(Modal::CloseEditor);
        } else {
            self.editor = None;
            self.editor_full = false;
        }
    }

    pub fn external_editor(&self) -> String {
        let explicit = self.editor_cmd.as_deref().or(self.snapshot.editor.as_deref());
        crate::split::editor_command(explicit)
    }

    pub fn edit_selected_external(&mut self) {
        let Some(path) = self.current_file() else {
            self.toast("select a file first", true);
            return;
        };
        let workdir = self.snapshot.path.clone();
        if !workdir.join(&path).exists() {
            self.toast(format!("{path} is not in the working tree"), true);
            return;
        }
        let editor = self.external_editor();
        match crate::split::open_editor(&workdir, &path, &editor) {
            Ok(()) => self.toast(format!("opened {path} in {editor}"), false),
            Err(e) => self.toast(format!("cannot open editor: {e:#}"), true),
        }
    }

    pub fn preview_selected_in_cmux(&mut self) {
        let Some(path) = self.current_file() else {
            self.toast("select a file first", true);
            return;
        };
        let workdir = self.snapshot.path.clone();
        if !workdir.join(&path).exists() {
            self.toast(format!("{path} is not in the working tree"), true);
            return;
        }
        match crate::split::cmux_open(&workdir, &path) {
            Ok(()) => self.toast(format!("opened {path} in cmux"), false),
            Err(e) => self.toast(format!("{e:#}"), true),
        }
    }

    // ---- update ----

    pub fn update(&mut self, msg: Message) {
        let had_modal = self.modal.is_some();
        self.update_inner(msg);
        if !had_modal && self.modal.is_some() {
            // A dialog owns the keyboard: drop focus from the editors and
            // inputs underneath before the dialog's own field takes it.
            self.ops.insert(0, Box::new(iced_core::widget::operation::focusable::unfocus()));
        }
    }

    fn update_inner(&mut self, msg: Message) {
        match msg {
            Message::Nothing => {}
            Message::Key(key, mods) => self.key(key, mods),
            Message::PaneClicked(p) => {
                if let Some(kind) = self.active_panes().get(p) {
                    self.focus = *kind;
                }
            }
            Message::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.active_panes_mut().drop(pane, target);
            }
            Message::PaneDragged(_) => {}
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.active_panes_mut().resize(split, ratio);
            }
            Message::PaneMaximize(p) => self.active_panes_mut().maximize(p),
            Message::PaneRestore => self.active_panes_mut().restore(),
            Message::SelectRow(sel) => {
                self.focus = Pane::Log;
                self.select(sel);
            }
            Message::SelectFile(t) => {
                self.focus = Pane::Changes;
                if let Some(ed) = &self.editor {
                    if !ed.dirty() && ed.path != t.path() {
                        // Fall through: follow_selection_in_editor handles it.
                    }
                }
                self.select_file(Some(t));
            }
            Message::SidebarSelect(name, oid) => {
                self.sidebar_selected = Some(name);
                self.focus = Pane::Sidebar;
                if let Some(idx) = self.snapshot.commits.iter().position(|c| c.oid == oid) {
                    self.select(Selection::Commit(idx));
                    self.scroll_to_selection.set(true);
                } else {
                    self.toast(
                        format!("{} is not in the loaded log", crate::git::repo::short_id(oid)),
                        false,
                    );
                }
            }
            Message::FilterChanged(s) => {
                self.filter = s;
                self.filter_active = true;
                self.rebuild_filter();
            }
            Message::FilterClear => {
                self.filter.clear();
                self.filter_active = false;
                self.rebuild_filter();
            }
            Message::FilterOpen => {
                self.filter_active = true;
                self.focus = Pane::Log;
                self.ops.push(Box::new(iced_core::widget::operation::focusable::focus(
                    widgets::FILTER_ID.clone(),
                )));
            }
            Message::Run(cmd) => self.run(cmd),
            Message::Refresh => {
                self.pending.push(Command::Refresh);
                self.toast("refreshing", false);
            }
            Message::Switch(name) => self.try_switch_branch(name),
            Message::CheckoutDetached(oid) => {
                if self.snapshot.is_dirty() {
                    self.toast("commit or stash your changes before checking out a commit", true);
                } else {
                    self.run(Command::CheckoutDetached(oid));
                }
            }
            Message::Discard(paths) => {
                if self.busy == 0 {
                    self.modal = Some(Modal::Discard(paths));
                }
            }
            Message::DiscardAll => self.discard_all(),
            Message::Ignore(p) => self.input(InputKind::Ignore, format!("/{p}"), String::new()),
            Message::Resolve(path, side) => self.run(Command::Resolve { path, side }),
            Message::Commit => self.commit_now(false),
            Message::CommitAndPush => self.commit_now(true),
            Message::CommitMsg(action) => self.commit_msg.perform(action),
            Message::ToggleAmend(on) => {
                self.amend = on;
                if on && !self.amend_loaded {
                    if let Some(m) = self.snapshot.head_message.clone() {
                        if self.commit_message_text().trim().is_empty() {
                            self.set_commit_message(&m);
                        }
                    }
                    self.amend_loaded = true;
                }
            }
            Message::CommitAction(idx, action) => self.commit_action(idx, action),
            Message::StateAction(a) => self.state_action(a),
            Message::DiffSearch(s) => {
                self.diff_search = s;
                self.diff_match = 0;
                self.diff_jump.set(true);
            }
            Message::DiffSearchOpen => {
                self.diff_search_active = true;
                self.focus = Pane::Detail;
                self.ops.push(Box::new(iced_core::widget::operation::focusable::focus(
                    widgets::DIFF_SEARCH_ID.clone(),
                )));
            }
            Message::DiffSearchClose => {
                self.diff_search_active = false;
                self.diff_search.clear();
                self.diff_match = 0;
            }
            Message::DiffNext(dir) => self.diff_next_match(dir),
            Message::DiffContext(d) => self.change_diff_context(d),
            Message::DiffWhitespace => self.toggle_whitespace(),
            Message::DiffWrap => self.wrap = !self.wrap,
            Message::EditorWrap => self.editor_wrap = !self.editor_wrap,
            Message::DiffLineClick { hunk, line, shift } => {
                self.focus = Pane::Detail;
                match self.line_sel {
                    Some(sel) if shift && sel.hunk == hunk => {
                        self.line_sel = Some(LineSel {
                            hunk,
                            anchor: sel.anchor,
                            end: line,
                        });
                    }
                    Some(sel) if !shift && sel.hunk == hunk && sel.anchor == line && sel.end == line => {
                        self.line_sel = None;
                    }
                    _ => {
                        self.line_sel = Some(LineSel {
                            hunk,
                            anchor: line,
                            end: line,
                        });
                    }
                }
            }
            Message::DiffDragTo { hunk, line } => {
                if let Some(sel) = self.line_sel {
                    if sel.hunk == hunk {
                        self.line_sel = Some(LineSel { end: line, ..sel });
                    }
                }
            }
            Message::DiffHunk(action, hunk_index) => {
                let Some(d) = &self.diff else { return };
                let path = d.target.path().to_owned();
                match action {
                    HunkAction::Stage => self.run(Command::StageHunk { path, hunk_index }),
                    HunkAction::Unstage => self.run(Command::UnstageHunk { path, hunk_index }),
                    HunkAction::Discard => self.confirm(
                        "Discard hunk",
                        format!("Throw away this hunk of {path}? This cannot be undone."),
                        "Discard",
                        Command::DiscardHunk { path, hunk_index },
                    ),
                }
            }
            Message::LinesStage => {
                self.stage_selected_lines();
            }
            Message::LinesUnstage => {
                self.unstage_selected_lines();
            }
            Message::LinesDiscard => {
                self.discard_selected_lines();
            }
            Message::ClearLineSel => self.line_sel = None,
            Message::ModalClose => self.modal = None,
            Message::ModalConfirm => self.modal_confirm(),
            Message::ModalValue(v) => self.modal_value(v),
            Message::ModalExtra(v) => match &mut self.modal {
                Some(Modal::Input { extra, .. }) => *extra = v,
                Some(Modal::PublishGithub { description, .. }) => *description = v,
                _ => {}
            },
            Message::ModalMultiline(action) => self.modal_multiline.perform(action),
            Message::ModalCheckbox(on) => match &mut self.modal {
                Some(Modal::NewBranch { checkout, .. }) => *checkout = on,
                Some(Modal::StashOpts { keep_index, .. }) => *keep_index = on,
                Some(Modal::PublishGithub { private, .. }) => *private = on,
                _ => {}
            },
            Message::ModalCheckbox2(on) => {
                if let Some(Modal::StashOpts {
                    include_untracked, ..
                }) = &mut self.modal
                {
                    *include_untracked = on;
                }
            }
            Message::ModalReset(kind) => {
                if let Some(Modal::Reset { oid, .. }) = self.modal.clone() {
                    self.modal = None;
                    self.run(Command::Reset { oid, kind });
                }
            }
            Message::ModalCheckoutStash => {
                if let Some(Modal::CheckoutConfirm { target }) = self.modal.clone() {
                    self.modal = None;
                    let message = self.stash_message_for_switch(&target);
                    self.run(Command::StashAndCheckout {
                        branch: target,
                        message,
                    });
                }
            }
            Message::ModalCheckoutForce => {
                if let Some(Modal::CheckoutConfirm { target }) = self.modal.clone() {
                    self.modal = None;
                    self.run(Command::ForceCheckout(target));
                }
            }
            Message::ModalPick(name) => self.try_switch_branch(name),
            Message::ModalEditorSave => {
                self.save_editor();
                if self.editor.as_ref().is_some_and(|e| !e.dirty()) {
                    self.editor = None;
                        }
                self.modal = None;
            }
            Message::ModalEditorDiscard => {
                self.editor = None;
                    self.modal = None;
            }
            Message::OpenBranchPicker => {
                if self.busy == 0 {
                    self.modal = Some(Modal::BranchPicker {
                        filter: String::new(),
                    });
                    self.focus_modal_input();
                }
            }
            Message::OpenPublish => {
                if self.busy == 0 && !self.has_origin() {
                    self.modal = Some(Modal::PublishGithub {
                        name: self.default_github_repo_name(),
                        description: String::new(),
                        private: false,
                    });
                    self.focus_modal_input();
                }
            }
            Message::OpenHelp => self.modal = Some(Modal::Help),
            Message::OpenStateMenu => {
                if self.snapshot.state != RepoState::Clean && self.busy == 0 {
                    self.modal = Some(Modal::StateMenu);
                }
            }
            Message::OpenStashDialog => {
                if self.snapshot.is_dirty() && self.busy == 0 {
                    self.modal = Some(Modal::StashOpts {
                        message: String::new(),
                        keep_index: false,
                        include_untracked: true,
                    });
                    self.focus_modal_input();
                }
            }
            Message::OpenNewBranch => {
                let Some(head) = self.snapshot.head.as_ref().and_then(|h| h.oid) else { return };
                let label = self
                    .snapshot
                    .head
                    .as_ref()
                    .and_then(|h| h.branch_name.clone())
                    .unwrap_or_else(|| crate::git::repo::short_id(head));
                self.modal = Some(Modal::NewBranch {
                    name: String::new(),
                    from: head,
                    from_label: label,
                    checkout: true,
                });
                self.focus_modal_input();
            }
            Message::MenuOpen(kind) => {
                self.menu = Some(Menu {
                    at: self.cursor,
                    kind,
                });
            }
            Message::MenuClose => self.menu = None,
            Message::MenuPick(inner) => {
                self.menu = None;
                self.update(*inner);
            }
            Message::Input(kind, value, extra) => self.input(kind, value, extra),
            Message::Confirm(title, body, button, cmd) => self.confirm(title, body, button, cmd),
            Message::Modal(m) => {
                if self.busy == 0 {
                    let wants_input = matches!(m, Modal::NewBranch { .. });
                    self.modal = Some(m);
                    if wants_input {
                        self.focus_modal_input();
                    }
                }
            }
            Message::Copy(s) => {
                self.copy(s);
                self.toast("copied", false);
            }
            Message::PullRequest(branch) => {
                let url = self
                    .web_remote()
                    .and_then(|r| crate::git::actions::pull_request_url(r, &branch));
                match url {
                    Some(u) => self.open_url(&u),
                    None => self.toast("no web remote for this repository", true),
                }
            }
            Message::TreeToggle(d) => {
                self.tree_selected = Some(d.clone());
                self.focus = Pane::Sidebar;
                self.toggle_dir(&d);
            }
            Message::TreeOpen(p) => {
                self.tree_selected = Some(p.clone());
                self.focus = Pane::Sidebar;
                self.open_editor(p, true);
            }
            Message::TreeRequest(d) => {
                self.tree_requested.remove(&d);
                self.request_dir(&d);
            }
            Message::ShowChanges(path) => {
                self.tree_selected = Some(path.clone());
                if self.editor.as_ref().is_some_and(|e| !e.dirty()) {
                    self.editor = None;
                        }
                let target = if self.snapshot.unstaged.iter().any(|f| f.path == path) {
                    DiffTarget::WorkdirUnstaged(path)
                } else {
                    DiffTarget::Staged(path)
                };
                self.select(Selection::WorkingTree);
                self.select_file(Some(target));
                self.focus = Pane::Detail;
            }
            Message::MergeOpen(path) => self.open_merge(path),
            Message::MergeSet(i, r) => {
                if let Some(m) = self.merge.as_mut() {
                    m.set(i, r);
                }
            }
            Message::MergeAll(r) => {
                if let Some(m) = self.merge.as_mut() {
                    m.set_all(r);
                }
            }
            Message::MergeApply => {
                let Some(m) = self.merge.as_ref() else { return };
                if m.resolved() < m.conflicts() {
                    self.toast("resolve every conflict first", true);
                    return;
                }
                match m.write() {
                    Ok(()) => {
                        let path = m.path.clone();
                        self.merge = None;
                        self.run(Command::Stage(vec![path.clone()]));
                        self.toast(format!("{path} resolved"), false);
                    }
                    Err(e) => self.toast(format!("cannot write: {e}"), true),
                }
            }
            Message::MergeEdit => {
                let Some(m) = self.merge.as_ref() else { return };
                let path = m.path.clone();
                match m.write() {
                    Ok(()) => {
                        self.merge = None;
                        self.open_editor(path, false);
                    }
                    Err(e) => self.toast(format!("cannot write: {e}"), true),
                }
            }
            Message::MergeClose => self.merge = None,
            Message::Edit => {
                let Some(path) = self.current_file() else {
                    self.toast("select a file first", true);
                    return;
                };
                let from_tree = self.focus == Pane::Sidebar && self.tree_selected.as_deref() == Some(path.as_str());
                self.open_editor(path, from_tree);
            }
            Message::EditorAction(action) => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.perform(action);
                }
            }
            Message::EditorSave => self.save_editor(),
            Message::EditorClose => self.close_editor(),
            Message::EditorExternal => self.edit_selected_external(),
            Message::EditorPreview => self.preview_selected_in_cmux(),
            Message::SectionToggle(title) => {
                if !self.sidebar_collapsed.remove(title) {
                    self.sidebar_collapsed.insert(title);
                }
            }
            Message::NetClose => self.net.open = false,
            Message::Quit => self.quit = true,
            Message::InitRepo => self.run(Command::InitRepo),
        }
    }

    fn modal_value(&mut self, v: String) {
        match &mut self.modal {
            Some(Modal::Input { value, .. }) => *value = v,
            Some(Modal::NewBranch { name, .. }) => *name = v,
            Some(Modal::BranchPicker { filter }) => *filter = v,
            Some(Modal::PublishGithub { name, .. }) => *name = v,
            Some(Modal::StashOpts { message, .. }) => *message = v,
            _ => {}
        }
    }

    /// Enter or the primary button of the open dialog.
    fn modal_confirm(&mut self) {
        let Some(modal) = self.modal.clone() else { return };
        match modal {
            Modal::Discard(paths) => {
                self.modal = None;
                self.run(Command::Discard(paths));
            }
            Modal::NewBranch {
                name, from, checkout, ..
            } => {
                let n = name.trim().to_owned();
                if n.is_empty() || n.contains(' ') {
                    return;
                }
                self.modal = None;
                self.run(Command::CreateBranch {
                    name: n,
                    from,
                    checkout,
                });
            }
            Modal::DeleteBranch(name) => {
                self.modal = None;
                self.run(Command::DeleteBranch(name));
            }
            Modal::DropStash(i) => {
                self.modal = None;
                self.run(Command::StashDrop(i));
            }
            Modal::BranchPicker { filter } => {
                let pick = self
                    .snapshot
                    .branches
                    .iter()
                    .find(|b| modal::branch_matches(&b.name, &filter))
                    .map(|b| b.name.clone());
                if let Some(name) = pick {
                    self.try_switch_branch(name);
                }
            }
            Modal::CheckoutConfirm { .. } => self.update(Message::ModalCheckoutStash),
            Modal::PublishGithub {
                name,
                description,
                private,
            } => {
                if !Self::valid_github_repo_name(&name) {
                    return;
                }
                self.modal = None;
                self.run(Command::PublishGithub {
                    name: name.trim().to_owned(),
                    description: description.trim().to_owned(),
                    private,
                });
            }
            Modal::Confirm { cmd, .. } => {
                self.modal = None;
                self.run(cmd);
            }
            Modal::Input { kind, value, extra } => {
                let value = if matches!(kind, InputKind::Reword { .. }) {
                    self.modal_multiline.text()
                } else {
                    value
                };
                if !kind.valid(&value, &extra) {
                    return;
                }
                self.modal = None;
                self.run(kind.command(&value, &extra));
            }
            Modal::Reset { .. } => self.update(Message::ModalReset(ResetKind::Mixed)),
            Modal::StashOpts {
                message,
                keep_index,
                include_untracked,
            } => {
                self.modal = None;
                self.run(Command::StashPushOpts {
                    message: message.trim().to_owned(),
                    keep_index,
                    include_untracked,
                });
            }
            Modal::StateMenu => self.state_action(StateAction::Continue),
            Modal::Help => self.modal = None,
            Modal::CloseEditor => self.update(Message::ModalEditorSave),
        }
    }

    // ---- keyboard ----

    fn key(&mut self, key: keyboard::Key, mods: keyboard::Modifiers) {
        let ctrl = mods.control();
        let shift = mods.shift();
        let plain = !ctrl && !mods.alt() && !mods.logo();
        let ch = match &key {
            keyboard::Key::Character(s) => s.as_str(),
            _ => "",
        };
        let named = match &key {
            keyboard::Key::Named(n) => Some(*n),
            _ => None,
        };

        // A dialog owns the keyboard.
        if self.modal.is_some() {
            match named {
                Some(Named::Escape) => self.modal = None,
                Some(Named::Enter) => self.modal_confirm(),
                _ => {}
            }
            return;
        }
        if self.menu.is_some() {
            if named == Some(Named::Escape) {
                self.menu = None;
            }
            return;
        }
        if named == Some(Named::Escape) {
            if self.merge.is_some() {
                self.merge = None;
            } else if self.editor.is_some() {
                self.close_editor();
            } else if self.line_sel.is_some() {
                self.line_sel = None;
            } else if self.diff_search_active {
                self.update(Message::DiffSearchClose);
            } else if self.filter_active {
                self.update(Message::FilterClear);
            }
            return;
        }
        if ctrl {
            match ch {
                "c" => self.quit = true,
                "f" => self.update(Message::DiffSearchOpen),
                "w" => self.toggle_whitespace(),
                "d" => self.show_debug = !self.show_debug,
                "s" => self.save_editor(),
                _ => {}
            }
            if named == Some(Named::Enter) {
                self.commit_now(shift);
            }
            return;
        }
        if !plain {
            return;
        }
        let commit = self.selected_commit();
        let searching = !self.diff_search.is_empty();
        match named {
            Some(Named::ArrowDown) => return self.nav(1),
            Some(Named::ArrowUp) => return self.nav(-1),
            Some(Named::PageDown) => return self.nav(20),
            Some(Named::PageUp) => return self.nav(-20),
            Some(Named::Home) => return self.nav(-1_000_000),
            Some(Named::End) => return self.nav(1_000_000),
            Some(Named::Tab) => {
                self.focus = match self.focus {
                    Pane::Sidebar => Pane::Log,
                    Pane::Log => Pane::Changes,
                    Pane::Changes => Pane::Detail,
                    Pane::Detail => Pane::Sidebar,
                };
                return;
            }
            Some(Named::Enter) => {
                if self.focus == Pane::Sidebar {
                    if let Some(name) = self.sidebar_selected.clone() {
                        if self.snapshot.branches.iter().any(|b| b.name == name) {
                            self.try_switch_branch(name);
                        }
                    }
                }
                return;
            }
            Some(Named::Space) => return self.toggle_stage_selected(),
            _ => {}
        }
        match ch {
            "?" => self.modal = Some(Modal::Help),
            "j" => self.nav(1),
            "k" => self.nav(-1),
            "/" => self.update(Message::FilterOpen),
            "s" => {
                if !self.stage_selected_lines() {
                    self.stage_selected();
                }
            }
            "u" => {
                if !self.unstage_selected_lines() {
                    self.unstage_selected();
                }
            }
            "a" => self.run(Command::StageAll),
            "A" => self.run(Command::UnstageAll),
            "S" => self.update(Message::OpenStashDialog),
            "D" => self.discard_all(),
            "i" => {
                if let Some((p, false)) = self.selected_worktree_file() {
                    let untracked = self
                        .snapshot
                        .unstaged
                        .iter()
                        .any(|f| f.path == p && f.kind == crate::git::repo::FileKind::Untracked);
                    if untracked {
                        self.input(InputKind::Ignore, format!("/{p}"), String::new());
                    } else {
                        self.toast("only untracked files can be ignored", true);
                    }
                }
            }
            "d" => match commit {
                Some(idx) => self.commit_rewrite(idx, TodoAction::Drop),
                None => {
                    if !self.discard_selected_lines() {
                        if let Some((p, false)) = self.selected_worktree_file() {
                            if self.busy == 0 {
                                self.modal = Some(Modal::Discard(vec![p]));
                            }
                        }
                    }
                }
            },
            "n" => {
                if searching {
                    self.diff_next_match(1);
                } else if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::NewBranch);
                }
            }
            "N" => {
                if searching {
                    self.diff_next_match(-1);
                }
            }
            "T" => {
                if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::Tag);
                }
            }
            "t" => {
                if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::Revert);
                }
            }
            "C" => {
                if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::CherryPick);
                }
            }
            "g" => {
                if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::Reset);
                }
            }
            "R" => {
                if let Some(idx) = commit {
                    self.commit_reword(idx);
                }
            }
            "K" => {
                if let Some(idx) = commit {
                    self.commit_rewrite(idx, TodoAction::MoveUp);
                }
            }
            "J" => {
                if let Some(idx) = commit {
                    self.commit_rewrite(idx, TodoAction::MoveDown);
                }
            }
            "y" => {
                if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::CopyHash);
                }
            }
            "o" => {
                if let Some(idx) = commit {
                    self.commit_action(idx, CommitAction::OpenBrowser);
                }
            }
            "m" => self.update(Message::OpenStateMenu),
            "e" => self.update(Message::Edit),
            "E" => self.edit_selected_external(),
            "O" => self.preview_selected_in_cmux(),
            "{" => self.change_diff_context(-1),
            "}" => self.change_diff_context(1),
            "c" => {
                self.selection = Selection::WorkingTree;
                self.focus = Pane::Changes;
                self.focus_commit_box();
            }
            "f" => self.run(Command::Fetch),
            "p" => self.run(Command::Pull),
            "P" => self.run(Command::Push),
            "r" => self.update(Message::Refresh),
            "q" => self.quit = true,
            "1" | "2" | "3" | "4" => {
                let kind = match ch {
                    "1" => Pane::Sidebar,
                    "2" => Pane::Log,
                    "3" => Pane::Changes,
                    _ => Pane::Detail,
                };
                if let Some(p) = self.pane_of(kind) {
                    let panes = self.active_panes_mut();
                    if panes.maximized() == Some(p) {
                        panes.restore();
                    } else {
                        panes.maximize(p);
                    }
                }
            }
            _ => {}
        }
    }

    fn nav(&mut self, delta: i32) {
        if matches!(self.focus, Pane::Log | Pane::Sidebar | Pane::Changes) {
            self.move_selection(delta);
        }
    }

    // ---- view ----

    pub fn view(&self) -> Element<'_> {
        if self.no_repo {
            return self.view_no_repo();
        }
        let hovered = self.hovered_pane();
        let grid = pane_grid_widget(self.active_panes(), |pane, kind, maximized| {
            let body: Element<'_> = match kind {
                Pane::Sidebar => sidebar::view(self),
                Pane::Log => log::view(self),
                Pane::Changes => changes::view(self),
                Pane::Detail => {
                    if self.merge.is_some() {
                        merge::view(self)
                    } else if self.editor.is_some() {
                        editor::view(self)
                    } else {
                        diff::view(self)
                    }
                }
            };
            let title = match kind {
                Pane::Detail if self.merge.is_some() => "Resolve conflicts",
                Pane::Detail if self.editor.is_some() => "Editor",
                k => k.title(),
            };
            widgets::pane(self, pane, title, *kind == self.focus, maximized, hovered == Some(pane), body)
        })
        .spacing(widgets::PANE_SPACING)
        .min_size(widgets::PANE_MIN)
        .on_click(Message::PaneClicked)
        .on_drag(Message::PaneDragged)
        .on_resize(8, Message::PaneResized)
        .style(widgets::pane_grid_style);

        let mut main = column![].spacing(0);
        main = main.push(container(widgets::grid_frame(self, grid)).width(Length::Fill).height(Length::Fill).padding(4));
        if self.net.open {
            main = main.push(footer::net_log(self));
        }
        main = main.push(footer::view(self));
        let base: Element<'_> = main.into();
        let mut layers = stack![base];
        if let Some(m) = &self.menu {
            layers = layers.push(widgets::layered(menu::view(self, m)));
        }
        if let Some(m) = &self.modal {
            layers = layers.push(widgets::layered(modal::view(self, m)));
        }
        if !self.toasts.is_empty() {
            layers = layers.push(widgets::layered(widgets::toasts(self)));
        }
        layers.into()
    }

    fn view_no_repo(&self) -> Element<'_> {
        let body = column![
            text("Not a git repository").size(20),
            text(self.repo_path.display().to_string()).font(iced_core::Font::MONOSPACE),
            text("Initialize a repository here to start using gitgui."),
            widgets::button("Initialize git repository", (self.busy == 0).then_some(Message::InitRepo)),
        ]
        .spacing(10)
        .align_x(iced_core::Alignment::Center);
        let content = column![
            iced_widget::center(body).width(Length::Fill).height(Length::Fill),
            footer::view(self)
        ];
        let mut layers = stack![Element::from(content)];
        if !self.toasts.is_empty() {
            layers = layers.push(widgets::layered(widgets::toasts(self)));
        }
        layers.into()
    }
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
        assert_eq!(age(100, 70), "30s");
        assert_eq!(age(1000, 0), "16m");
        assert_eq!(age(10_000, 0), "2h");
        assert_eq!(age(200_000, 0), "2d");
        assert_eq!(age(5_000_000, 0), "1mo");
        assert_eq!(age(40_000_000, 0), "1y");
    }
}
