//! Detail pane: commit files or working tree lists, plus the diff viewer.

use crate::git::actions::ConflictSide;
use crate::git::ops::Command;
use crate::git::repo::{DiffTarget, FileKind, FileStatus, RepoState};
use crate::ui::app::{App, InputKind, Modal, Pane, Selection};
use crate::ui::diff;

pub fn show_detail(app: &mut App, ui: &mut egui::Ui) {
    let focused = app.focus == Pane::Detail;
    let avail = ui.available_width();
    egui::Panel::left("detail_files")
        .default_size((avail * 0.35).clamp(200.0, 480.0))
        .resizable(true)
        .show(ui, |ui| match app.selection {
            Selection::WorkingTree => show_worktree(app, ui, focused),
            Selection::Commit(i) => show_commit(app, ui, i, focused),
        });
    egui::CentralPanel::default().show(ui, |ui| diff::show(app, ui));
}

fn file_row(ui: &mut egui::Ui, f: &FileStatus, selected: bool, theme: &crate::ui::theme::Theme) -> egui::Response {
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    let color = match f.kind {
        FileKind::Added | FileKind::Untracked => theme.add_fg,
        FileKind::Deleted => theme.del_fg,
        FileKind::Conflicted => theme.error,
        _ => ui.visuals().text_color(),
    };
    let label = match &f.old_path {
        Some(old) => format!("{} {} -> {}", f.kind.letter(), old, f.path),
        None => format!("{} {}", f.kind.letter(), f.path),
    };
    ui.selectable_label(selected, egui::RichText::new(label).color(color).monospace())
}

fn show_commit(app: &mut App, ui: &mut egui::Ui, idx: usize, _focused: bool) {
    let Some(c) = app.snapshot.commits.get(idx).cloned() else { return };
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.monospace(&c.short);
        ui.strong(&c.author);
        ui.weak(&c.email);
        ui.weak(format_time(c.time));
    });
    ui.add(egui::Label::new(egui::RichText::new(&c.summary).strong()).wrap());
    ui.separator();
    let files = app.commit_files.get(&c.oid).cloned();
    let mut clicked: Option<DiffTarget> = None;
    // The message body and the file list scroll together: a long body must
    // not push the files out of a short pane, and a long file list must not
    // hide the body.
    egui::ScrollArea::vertical()
        .id_salt("commit_files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !c.body.is_empty() {
                ui.add(
                    egui::Label::new(egui::RichText::new(&c.body).monospace())
                        .wrap(),
                );
                ui.add_space(4.0);
                ui.separator();
            }
            match files {
                None => {
                    ui.weak("loading files");
                }
                Some(files) => {
                    if files.is_empty() {
                        ui.weak("no changes");
                    }
                    for f in &files {
                        let target = DiffTarget::Commit(c.oid, f.path.clone());
                        let selected = app.selected_file.as_ref() == Some(&target);
                        if file_row(ui, f, selected, &app.theme).clicked() {
                            clicked = Some(target);
                        }
                    }
                }
            }
        });
    if let Some(t) = clicked {
        app.focus = Pane::Detail;
        app.select_file(Some(t));
    }
}

struct WorktreeListAction {
    clicked: Option<DiffTarget>,
    cmd: Option<Command>,
    modal: Option<Modal>,
    ignore: Option<String>,
    copy: Option<String>,
    discard_all: bool,
}

fn show_worktree(app: &mut App, ui: &mut egui::Ui, _focused: bool) {
    let s = app.snapshot.clone();
    let busy = app.busy > 0;
    let mut action = WorktreeListAction {
        clicked: None,
        cmd: None,
        modal: None,
        ignore: None,
        copy: None,
        discard_all: false,
    };

    // Hard split: the commit box owns the bottom `commit_h` points no matter
    // how tall the lists want to be, so it never gets pushed off screen in a
    // short pane. Each half is clipped to its own rect.
    let mut full = ui.available_rect_before_wrap();
    if ui.is_sizing_pass() || !full.height().is_finite() {
        // The panel measures its content before it has a stored size; report
        // a modest height instead of filling whatever it offers.
        full.max.y = full.min.y + 260.0;
    }
    let total_h = full.height();
    let commit_h = commit_box_height(total_h).min(total_h);
    let lists_h = (total_h - commit_h).max(0.0);
    let lists_rect =
        egui::Rect::from_min_size(full.min, egui::vec2(full.width(), lists_h));
    let commit_rect = egui::Rect::from_min_max(
        egui::pos2(full.min.x, full.max.y - commit_h),
        full.max,
    );

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(lists_rect)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
        |ui| {
            ui.set_clip_rect(lists_rect.intersect(ui.clip_rect()));
            show_worktree_lists(app, ui, &s, lists_h, busy, &mut action);
        },
    );

    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(commit_rect)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
        |ui| {
            ui.set_clip_rect(commit_rect.intersect(ui.clip_rect()));
            show_commit_box(app, ui, &s, commit_h, busy);
        },
    );

    if let Some(t) = action.clicked {
        app.focus = Pane::Detail;
        app.select_file(Some(t));
    }
    if let Some(c) = action.cmd {
        app.run(c);
    }
    if action.modal.is_some() && app.busy == 0 {
        app.modal = action.modal;
    }
    if let Some(p) = action.ignore {
        app.input(InputKind::Ignore, format!("/{p}"), String::new());
    }
    if let Some(p) = action.copy {
        ui.ctx().copy_text(p);
        app.toast("copied path", false);
    }
    if action.discard_all {
        app.discard_all();
    }
}

/// Right-click menu of a working tree file row.
fn file_menu(ui: &mut egui::Ui, f: &FileStatus, staged: bool, busy: bool, action: &mut WorktreeListAction) {
    let path = f.path.clone();
    let item = |ui: &mut egui::Ui, enabled: bool, label: &str, tip: &str| -> bool {
        let r = ui.add_enabled(enabled && !busy, egui::Button::new(label));
        let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
        let clicked = r.clicked();
        if clicked {
            ui.close();
        }
        clicked
    };
    if f.kind == FileKind::Conflicted {
        if item(ui, true, "Use ours", "keep the version of the branch you are on (for a rebase: the upstream side)") {
            action.cmd = Some(Command::Resolve {
                path: path.clone(),
                side: ConflictSide::Ours,
            });
        }
        if item(ui, true, "Use theirs", "keep the incoming version (for a rebase: the commit being replayed)") {
            action.cmd = Some(Command::Resolve {
                path: path.clone(),
                side: ConflictSide::Theirs,
            });
        }
        if item(ui, true, "Mark resolved", "stage the file as it is in the working tree") {
            action.cmd = Some(Command::Stage(vec![path.clone()]));
        }
        ui.separator();
    } else if staged {
        if item(ui, true, "Unstage", "u") {
            action.cmd = Some(Command::Unstage(vec![path.clone()]));
        }
    } else {
        if item(ui, true, "Stage", "s") {
            action.cmd = Some(Command::Stage(vec![path.clone()]));
        }
        if item(ui, true, "Discard changes", "d, asks for confirmation") {
            action.modal = Some(Modal::Discard(vec![path.clone()]));
        }
        if f.kind == FileKind::Untracked && item(ui, true, "Add to .gitignore", "i") {
            action.ignore = Some(path.clone());
        }
    }
    if item(ui, true, "Copy path", "") {
        action.copy = Some(path);
    }
}

/// Space for the commit box at the bottom of the file list column.
pub fn commit_box_height(total_h: f32) -> f32 {
    if total_h <= 180.0 {
        72.0
    } else if total_h <= 260.0 {
        88.0
    } else {
        104.0
    }
}

/// Rows in the commit message field for the given box height.
/// Reserves space for the amend row and commit buttons below the message.
pub fn commit_message_rows(box_h: f32) -> usize {
    if box_h <= 96.0 {
        1
    } else {
        2
    }
}

/// Height of one file row in the unstaged / staged lists.
pub const FILE_ROW_H: f32 = 22.0;

/// Split the list column between the unstaged and staged scroll areas.
/// A list only takes what its rows need; the other list gets the rest, so
/// an empty "nothing staged" section does not waste half of a short pane.
pub fn list_scroll_heights(lists_h: f32, unstaged_rows: usize, staged_rows: usize) -> (f32, f32) {
    const UNSTAGED_CHROME: f32 = 52.0;
    const STAGED_CHROME: f32 = 28.0;
    const SEPARATOR: f32 = 8.0;
    let total = (lists_h - UNSTAGED_CHROME - STAGED_CHROME - SEPARATOR).max(0.0);
    let need = |rows: usize| rows.max(1) as f32 * FILE_ROW_H + 4.0;
    let (need_u, need_s) = (need(unstaged_rows), need(staged_rows));
    let half = total / 2.0;
    let u = need_u.min(total - need_s.min(half)).max(0.0);
    let s = (total - u).max(0.0);
    (u, s)
}

fn show_commit_box(app: &mut App, ui: &mut egui::Ui, s: &crate::git::repo::RepoSnapshot, box_h: f32, busy: bool) {
    ui.separator();
    let rows = commit_message_rows(box_h);
    let edit = egui::TextEdit::multiline(&mut app.commit_msg)
        .hint_text("Commit message")
        .desired_rows(rows)
        .desired_width(f32::INFINITY);
    let resp = ui.add(edit);
    if app.focus_commit_msg {
        resp.request_focus();
        app.focus_commit_msg = false;
    }
    let can_commit =
        !busy && (!s.staged.is_empty() || app.amend) && !app.commit_msg.trim().is_empty();
    let commit_tip = if can_commit {
        "Ctrl+Enter"
    } else if busy {
        "wait for the current operation"
    } else if app.commit_msg.trim().is_empty() {
        "enter a commit message"
    } else if s.staged.is_empty() && !app.amend {
        "stage files first"
    } else {
        "Ctrl+Enter"
    };
    let push_tip = if can_commit {
        "Ctrl+Shift+Enter"
    } else {
        commit_tip
    };
    // Buttons first, from the right; the amend checkbox takes what is left and
    // is clipped instead of overlapping in a narrow column. Both closures
    // need the app, so clicks are collected and applied afterwards.
    let amend = app.amend;
    let mut new_amend = amend;
    let clicked_commit = std::cell::Cell::new(false);
    let clicked_push = std::cell::Cell::new(false);
    let commit_rect = std::cell::Cell::new(None);
    let push_rect = std::cell::Cell::new(None);
    let author = format!("{} <{}>", s.user_name, s.user_email);
    crate::ui::row::split(
        ui,
        |ui| {
            let commit = ui
                .add_enabled(
                    can_commit,
                    egui::Button::new(if amend { "Amend" } else { "Commit" }).small(),
                )
                .on_hover_text(commit_tip);
            commit_rect.set(Some(commit.rect));
            clicked_commit.set(commit.clicked());
            let commit_push = ui
                .add_enabled(
                    can_commit,
                    egui::Button::new(if amend { "Amend & Push" } else { "Commit & Push" })
                        .small(),
                )
                .on_hover_text(push_tip);
            push_rect.set(Some(commit_push.rect));
            clicked_push.set(commit_push.clicked());
        },
        |ui| {
            // In a narrow column the label would truncate to a stray dot.
            let label = if ui.available_width() < 72.0 { "" } else { "Amend" };
            ui.checkbox(&mut new_amend, label)
                .on_hover_text(format!("Amend the last commit\n{author}"));
        },
    );
    app.commit_button_rect = commit_rect.get();
    app.commit_push_button_rect = push_rect.get();
    app.amend = new_amend;
    if new_amend && !amend && app.commit_msg.trim().is_empty() {
        app.amend_loaded = true;
        if let Some(m) = &s.head_message {
            app.commit_msg = m.trim_end().to_owned();
        }
    }
    if clicked_commit.get() {
        app.commit_now();
    } else if clicked_push.get() {
        app.commit_and_push_now();
    }
}

fn show_worktree_lists(
    app: &mut App,
    ui: &mut egui::Ui,
    s: &crate::git::repo::RepoSnapshot,
    lists_h: f32,
    busy: bool,
    action: &mut WorktreeListAction,
) {
    let (unstaged_h, staged_h) = list_scroll_heights(
        lists_h,
        s.unstaged.len() + s.conflicted.len(),
        s.staged.len(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(format!("Unstaged ({})", s.unstaged.len() + s.conflicted.len()));
        if s.state != RepoState::Clean {
            ui.colored_label(app.theme.error, format!("{} in progress", s.state.label()));
        }
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !s.unstaged.is_empty(), egui::Button::new("Stage all").small()).on_hover_text("a").clicked() {
            action.cmd = Some(Command::StageAll);
        }
        if ui.add_enabled(!busy && s.is_dirty(), egui::Button::new("Stash").small()).on_hover_text("Shift+S").clicked() {
            action.modal = Some(Modal::StashOpts {
                message: String::new(),
                keep_index: false,
                include_untracked: true,
            });
        }
        if let Some((p, false)) = app.selected_worktree_file() {
            if ui.add_enabled(!busy, egui::Button::new("Discard").small()).on_hover_text("d, asks for confirmation").clicked() {
                action.modal = Some(Modal::Discard(vec![p]));
            }
        }
        if ui
            .add_enabled(!busy && s.is_dirty(), egui::Button::new("Discard all").small())
            .on_hover_text("Shift+D, asks for confirmation")
            .clicked()
        {
            action.discard_all = true;
        }
    });
    egui::ScrollArea::vertical()
        .id_salt("unstaged")
        .max_height(unstaged_h)
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in s.conflicted.iter().chain(s.unstaged.iter()) {
                let target = DiffTarget::WorkdirUnstaged(f.path.clone());
                let selected = app.selected_file.as_ref() == Some(&target);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("+").small()).on_hover_text("stage (s)").clicked() {
                        action.cmd = Some(Command::Stage(vec![f.path.clone()]));
                    }
                    let resp = file_row(ui, f, selected, &app.theme);
                    if resp.clicked() || resp.secondary_clicked() {
                        action.clicked = Some(target);
                    }
                    resp.context_menu(|ui| file_menu(ui, f, false, busy, action));
                });
            }
            if s.unstaged.is_empty() && s.conflicted.is_empty() {
                ui.weak("nothing unstaged");
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(format!("Staged ({})", s.staged.len()));
        if ui.add_enabled(!busy && !s.staged.is_empty(), egui::Button::new("Unstage all").small()).on_hover_text("Shift+A").clicked() {
            action.cmd = Some(Command::UnstageAll);
        }
    });
    egui::ScrollArea::vertical()
        .id_salt("staged")
        .max_height(staged_h)
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &s.staged {
                let target = DiffTarget::Staged(f.path.clone());
                let selected = app.selected_file.as_ref() == Some(&target);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("-").small()).on_hover_text("unstage (u)").clicked() {
                        action.cmd = Some(Command::Unstage(vec![f.path.clone()]));
                    }
                    let resp = file_row(ui, f, selected, &app.theme);
                    if resp.clicked() || resp.secondary_clicked() {
                        action.clicked = Some(target);
                    }
                    resp.context_menu(|ui| file_menu(ui, f, true, busy, action));
                });
            }
            if s.staged.is_empty() {
                ui.weak("nothing staged");
            }
        });
}

pub fn format_time(t: i64) -> String {
    // Civil date from a unix timestamp, UTC, no chrono dependency.
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formatting() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
    }

    #[test]
    fn short_pane_heights_do_not_overlap_commit_box() {
        let total = 225.0;
        let commit = commit_box_height(total);
        let lists = total - commit;
        let (u, s) = list_scroll_heights(lists, 3, 3);
        const UNSTAGED_CHROME: f32 = 52.0;
        const STAGED_CHROME: f32 = 28.0;
        const SEPARATOR: f32 = 8.0;
        let used = UNSTAGED_CHROME + STAGED_CHROME + SEPARATOR + u + s + commit;
        assert!(
            used <= total + 0.5,
            "layout used {used} pt in a {total} pt pane"
        );
        assert_eq!(commit, 88.0);
        assert!(u >= 0.0 && s >= 0.0);
    }

    #[test]
    fn empty_list_yields_space_to_the_other() {
        let (u, s) = list_scroll_heights(200.0, 6, 0);
        assert!(s < u, "empty staged list took {s} pt, unstaged got {u}");
        assert!(u + s <= 200.0 - 88.0 + 0.5);
        let (u, s) = list_scroll_heights(200.0, 0, 6);
        assert!(u < s);
        let (u, s) = list_scroll_heights(200.0, 20, 20);
        assert!((u - s).abs() < 0.5, "both full lists split evenly: {u} {s}");
    }

    #[test]
    fn commit_box_shrinks_in_tiny_panes() {
        assert_eq!(commit_box_height(150.0), 72.0);
        assert_eq!(commit_message_rows(72.0), 1);
        assert_eq!(commit_message_rows(104.0), 2);
    }
}
