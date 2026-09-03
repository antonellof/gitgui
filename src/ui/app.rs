//! Top-level application state and layout (docs/SPEC.md section 4).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use git2::Oid;

use crate::git::ops::{Command, Reply};
use crate::git::repo::{DiffTarget, DiffText, FileStatus, RepoSnapshot};
use crate::ui::theme::Theme;
use crate::ui::{changes, diff, log, sidebar};

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
    NewBranch { name: String, from: Oid, from_label: String, checkout: bool },
    DeleteBranch(String),
    StashPush { message: String },
    DropStash(usize),
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
}

impl App {
    pub fn new(theme: Theme, transport: &'static str, scale: f32) -> Self {
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
            net: NetLog { label: "", lines: Vec::new(), running: false, open: false },
            focus_commit_msg: false,
            amend_loaded: false,
            busy: 0,
            commit_button_rect: None,
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        let text = text.into();
        self.last_op = Some(text.clone());
        self.toasts.push(Toast { text, error, at: Instant::now() });
    }

    /// Row count in the log including the virtual working tree row.
    pub fn has_worktree_row(&self) -> bool {
        self.snapshot.is_dirty() || self.snapshot.commits.is_empty()
    }

    pub fn apply(&mut self, reply: Reply) {
        match reply {
            Reply::Snapshot(s) => {
                let first = !self.have_snapshot;
                self.snapshot = s;
                self.have_snapshot = true;
                self.commit_files.clear();
                self.rebuild_filter();
                if first {
                    self.selection = if self.has_worktree_row() || self.snapshot.commits.is_empty() {
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
                            self.selection = if self.has_worktree_row() { Selection::WorkingTree } else { Selection::Commit(0) };
                            self.on_selection_changed();
                        }
                        Selection::WorkingTree => {
                            // File list changed: keep the file if still present, else pick first.
                            let still = self.selected_file.as_ref().is_some_and(|t| self.worktree_has(t));
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
                        let keep = self.selected_file.as_ref().is_some_and(|t| matches!(t, DiffTarget::Commit(o, _) if *o == oid));
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
                if self.net.running && matches!(label, "fetch" | "pull" | "push") {
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
        self.busy += 1;
        self.pending.push(cmd);
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
        self.run(Command::Commit { message: msg, amend: self.amend });
    }

    fn worktree_has(&self, t: &DiffTarget) -> bool {
        match t {
            DiffTarget::WorkdirUnstaged(p) => self.snapshot.unstaged.iter().chain(self.snapshot.conflicted.iter()).any(|f| &f.path == p),
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
                    c.summary.to_lowercase().contains(&q) || c.author.to_lowercase().contains(&q) || c.short.starts_with(&q)
                })
                .map(|(i, _)| i)
                .collect()
        };
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
        let first = s
            .unstaged
            .first()
            .map(|f| DiffTarget::WorkdirUnstaged(f.path.clone()))
            .or_else(|| s.staged.first().map(|f| DiffTarget::Staged(f.path.clone())))
            .or_else(|| s.conflicted.first().map(|f| DiffTarget::WorkdirUnstaged(f.path.clone())));
        self.select_file(first);
    }

    pub fn select_file(&mut self, target: Option<DiffTarget>) {
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
            // commit box, Escape leaves the field.
            if ctx.input(|i| ctrl(i, egui::Key::Enter)) && self.modal.is_none() {
                self.commit_now();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                ctx.memory_mut(|m| m.request_focus(egui::Id::NULL));
                if self.filter_active {
                    self.filter_active = false;
                    self.filter.clear();
                    self.rebuild_filter();
                }
            }
            return;
        }
        if self.modal.is_some() {
            // Dialogs handle their own keys.
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
        let (s_key, u_key, a_key, shift_a, c_key, f_key, p_key, shift_p, enter) = ctx.input(|i| {
            (
                plain(i, egui::Key::S),
                plain(i, egui::Key::U),
                plain(i, egui::Key::A),
                shifted(i, egui::Key::A),
                plain(i, egui::Key::C),
                plain(i, egui::Key::F),
                plain(i, egui::Key::P),
                shifted(i, egui::Key::P),
                plain(i, egui::Key::Enter),
            )
        });
        if s_key {
            self.stage_selected();
        }
        if u_key {
            self.unstage_selected();
        }
        if a_key {
            self.run(Command::StageAll);
        }
        if shift_a {
            self.run(Command::UnstageAll);
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
        if esc && self.filter_active {
            self.filter_active = false;
            self.filter.clear();
            self.rebuild_filter();
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
        self.handle_keys(&ctx);
        self.toasts.retain(|t| t.at.elapsed().as_secs_f32() < if t.error { 8.0 } else { 3.0 });
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::Panel::left("sidebar").default_size(220.0).resizable(true).show(root, |ui| {
            sidebar::show(self, ui);
        });
        egui::Panel::bottom("status").show(root, |ui| self.status_bar(ui));
        if self.net.open {
            egui::Panel::bottom("netlog").default_size(140.0).resizable(true).show(root, |ui| self.net_log(ui));
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

    fn net_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(format!("git {}", self.net.label));
            if self.net.running {
                ui.spinner();
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("close").clicked() {
                    self.net.open = false;
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("netlog_scroll").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for l in &self.net.lines {
                ui.monospace(l);
            }
        });
    }

    fn show_modal(&mut self, ctx: &egui::Context) {
        let Some(modal) = self.modal.clone() else { return };
        let mut close = false;
        let mut cmd: Option<Command> = None;
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        // Dim the background and swallow clicks outside the dialog.
        egui::Area::new(egui::Id::new("modal_dim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(ctx, |ui| {
                let rect = ctx.content_rect();
                ui.allocate_rect(rect, egui::Sense::click());
                ui.painter().rect_filled(rect, 0.0, egui::Color32::from_black_alpha(120));
            });
        let title = match &modal {
            Modal::Discard(_) => "Discard changes",
            Modal::NewBranch { .. } => "New branch",
            Modal::DeleteBranch(_) => "Delete branch",
            Modal::StashPush { .. } => "Stash changes",
            Modal::DropStash(_) => "Drop stash",
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
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
                    Modal::NewBranch { mut name, from, from_label, mut checkout } => {
                        ui.label(format!("From {from_label}"));
                        let resp = ui.add(egui::TextEdit::singleline(&mut name).hint_text("branch name").desired_width(f32::INFINITY));
                        if !resp.has_focus() && !ctx.egui_wants_keyboard_input() {
                            resp.request_focus();
                        }
                        ui.checkbox(&mut checkout, "check out after creating");
                        let valid = !name.trim().is_empty() && !name.contains(' ');
                        ui.horizontal(|ui| {
                            if ui.add_enabled(valid, egui::Button::new("Create")).clicked() || (enter && valid) {
                                cmd = Some(Command::CreateBranch { name: name.trim().to_owned(), from, checkout });
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if !close {
                            self.modal = Some(Modal::NewBranch { name, from, from_label, checkout });
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
                    Modal::StashPush { mut message } => {
                        let resp = ui.add(egui::TextEdit::singleline(&mut message).hint_text("stash message (optional)").desired_width(f32::INFINITY));
                        if !resp.has_focus() && !ctx.egui_wants_keyboard_input() {
                            resp.request_focus();
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Stash").clicked() || enter {
                                cmd = Some(Command::StashPush { message: message.clone() });
                                close = true;
                            }
                            if ui.button("Cancel").clicked() || esc {
                                close = true;
                            }
                        });
                        if !close {
                            self.modal = Some(Modal::StashPush { message });
                        }
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
                }
            });
        if let Some(c) = cmd {
            self.run(c);
        }
        if close {
            self.modal = None;
            ctx.memory_mut(|m| m.request_focus(egui::Id::NULL));
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let s = &self.snapshot;
            let path = s.path.to_string_lossy();
            let shown = match std::env::var("HOME") {
                Ok(h) if path.starts_with(&h) => format!("~{}", &path[h.len()..]),
                _ => path.to_string(),
            };
            ui.label(shown.trim_end_matches('/').to_owned());
            ui.separator();
            match &s.head {
                Some(h) => {
                    let name = h.branch_name.clone().unwrap_or_else(|| {
                        h.oid.map(|o| format!("detached {}", crate::git::repo::short_id(o))).unwrap_or_else(|| "no HEAD".into())
                    });
                    ui.strong(name);
                    if let Some(b) = s.branches.iter().find(|b| b.is_head) {
                        if b.ahead > 0 || b.behind > 0 {
                            ui.weak(format!("{}↑ {}↓", b.ahead, b.behind));
                        }
                    }
                }
                None => {
                    ui.weak("loading");
                }
            }
            ui.separator();
            ui.weak(format!("{} unstaged, {} staged", s.unstaged.len(), s.staged.len()));
            if let Some(op) = &self.last_op {
                ui.separator();
                ui.weak(op.clone());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.show_debug {
                    ui.weak(format!("{:.1} ms {} x{}", self.frame_ms, self.transport, self.scale));
                    ui.separator();
                }
                ui.weak("j/k move  s/u stage/unstage  a/A all  c commit  f/p/P fetch/pull/push  / filter  q quit");
            });
        });
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
                    let color = if t.error { self.theme.error } else { self.theme.ok };
                    egui::Frame::popup(ui.style()).stroke(egui::Stroke::new(1.0, color)).show(ui, |ui| {
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
        egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key => Some(*modifiers),
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
