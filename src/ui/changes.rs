//! Detail pane: commit files or working tree lists, plus the diff viewer.

use crate::git::ops::Command;
use crate::git::repo::{DiffTarget, FileKind, FileStatus};
use crate::ui::app::{App, Modal, Pane, Selection};
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
    ui.label(egui::RichText::new(&c.summary).strong());
    ui.separator();
    let files = app.commit_files.get(&c.oid).cloned();
    let mut clicked: Option<DiffTarget> = None;
    egui::ScrollArea::vertical().id_salt("commit_files").auto_shrink([false, false]).show(ui, |ui| match files {
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
}

fn show_worktree(app: &mut App, ui: &mut egui::Ui, _focused: bool) {
    let s = app.snapshot.clone();
    let busy = app.busy > 0;
    let mut action = WorktreeListAction {
        clicked: None,
        cmd: None,
        modal: None,
    };

    let total_h = ui.available_height();
    let commit_h = commit_box_height(total_h);
    let lists_h = (total_h - commit_h).max(0.0);

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), lists_h),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            show_worktree_lists(app, ui, &s, lists_h, busy, &mut action);
        },
    );

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), commit_h),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
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
    if action.modal.is_some() {
        app.modal = action.modal;
    }
}

/// Space for the commit box at the bottom of the file list column.
pub fn commit_box_height(total_h: f32) -> f32 {
    if total_h <= 180.0 {
        72.0
    } else if total_h <= 260.0 {
        96.0
    } else {
        118.0
    }
}

/// Rows in the commit message field for the given box height.
pub fn commit_message_rows(box_h: f32) -> usize {
    if box_h <= 80.0 {
        1
    } else if box_h <= 100.0 {
        2
    } else {
        3
    }
}

/// Split the list column between unstaged and staged scroll areas.
pub fn list_scroll_heights(lists_h: f32) -> (f32, f32) {
    const UNSTAGED_CHROME: f32 = 52.0;
    const STAGED_CHROME: f32 = 28.0;
    const SEPARATOR: f32 = 8.0;
    let scroll_total =
        (lists_h - UNSTAGED_CHROME - STAGED_CHROME - SEPARATOR).max(0.0);
    let each = scroll_total / 2.0;
    (each, each)
}

fn show_commit_box(app: &mut App, ui: &mut egui::Ui, s: &crate::git::repo::RepoSnapshot, box_h: f32, busy: bool) {
    ui.separator();
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let can_commit = !busy && (!s.staged.is_empty() || app.amend) && !app.commit_msg.trim().is_empty();
        let commit = ui
            .add_enabled(can_commit, egui::Button::new(if app.amend { "Amend" } else { "Commit" }))
            .on_hover_text("Ctrl+Enter");
        app.commit_button_rect = Some(commit.rect);
        if commit.clicked() {
            app.commit_now();
        }
        let commit_push = ui
            .add_enabled(
                can_commit,
                egui::Button::new(if app.amend {
                    "Amend & Push"
                } else {
                    "Commit & Push"
                }),
            )
            .on_hover_text("commit then git push");
        app.commit_push_button_rect = Some(commit_push.rect);
        if commit_push.clicked() {
            app.commit_and_push_now();
        }
        let before = app.amend;
        ui.checkbox(&mut app.amend, "amend");
        if app.amend && !before && app.commit_msg.trim().is_empty() {
            app.amend_loaded = true;
            if let Some(m) = &s.head_message {
                app.commit_msg = m.trim_end().to_owned();
            }
        }
        if box_h > 80.0 {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.weak(format!("{} <{}>", s.user_name, s.user_email));
        }
    });
    let rows = commit_message_rows(box_h);
    let edit = egui::TextEdit::multiline(&mut app.commit_msg)
        .hint_text("Commit message (Ctrl+Enter to commit)")
        .desired_rows(rows)
        .desired_width(f32::INFINITY);
    let resp = ui.add(edit);
    if app.focus_commit_msg {
        resp.request_focus();
        app.focus_commit_msg = false;
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
    let (unstaged_h, staged_h) = list_scroll_heights(lists_h);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(format!("Unstaged ({})", s.unstaged.len() + s.conflicted.len()));
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !s.unstaged.is_empty(), egui::Button::new("Stage all").small()).on_hover_text("a").clicked() {
            action.cmd = Some(Command::StageAll);
        }
        if ui.add_enabled(!busy && s.is_dirty(), egui::Button::new("Stash").small()).clicked() {
            action.modal = Some(Modal::StashPush { message: String::new() });
        }
        if let Some((p, false)) = app.selected_worktree_file() {
            if ui.add_enabled(!busy, egui::Button::new("Discard").small()).on_hover_text("asks for confirmation").clicked() {
                action.modal = Some(Modal::Discard(vec![p]));
            }
        }
    });
    egui::ScrollArea::vertical()
        .id_salt("unstaged")
        .max_height(unstaged_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in s.conflicted.iter().chain(s.unstaged.iter()) {
                let target = DiffTarget::WorkdirUnstaged(f.path.clone());
                let selected = app.selected_file.as_ref() == Some(&target);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("+").small()).on_hover_text("stage (s)").clicked() {
                        action.cmd = Some(Command::Stage(vec![f.path.clone()]));
                    }
                    if file_row(ui, f, selected, &app.theme).clicked() {
                        action.clicked = Some(target);
                    }
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
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &s.staged {
                let target = DiffTarget::Staged(f.path.clone());
                let selected = app.selected_file.as_ref() == Some(&target);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("-").small()).on_hover_text("unstage (u)").clicked() {
                        action.cmd = Some(Command::Unstage(vec![f.path.clone()]));
                    }
                    if file_row(ui, f, selected, &app.theme).clicked() {
                        action.clicked = Some(target);
                    }
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
        let (u, s) = list_scroll_heights(lists);
        const UNSTAGED_CHROME: f32 = 52.0;
        const STAGED_CHROME: f32 = 28.0;
        const SEPARATOR: f32 = 8.0;
        let used = UNSTAGED_CHROME + STAGED_CHROME + SEPARATOR + u + s + commit;
        assert!(
            used <= total + 0.5,
            "layout used {used} pt in a {total} pt pane"
        );
        assert_eq!(commit, 96.0);
        assert!(u >= 0.0 && s >= 0.0);
    }

    #[test]
    fn commit_box_shrinks_in_tiny_panes() {
        assert_eq!(commit_box_height(150.0), 72.0);
        assert_eq!(commit_message_rows(72.0), 1);
    }
}
