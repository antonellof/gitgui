//! Detail pane: commit files or working tree lists, plus the diff viewer.

use crate::git::repo::{DiffTarget, FileKind, FileStatus};
use crate::ui::app::{App, Pane, Selection};
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

fn show_worktree(app: &mut App, ui: &mut egui::Ui, _focused: bool) {
    let s = app.snapshot.clone();
    let mut clicked: Option<DiffTarget> = None;
    let half = (ui.available_height() - 60.0).max(80.0) / 2.0;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.strong(format!("Unstaged ({})", s.unstaged.len() + s.conflicted.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled(false, egui::Button::new("Stage all")).on_disabled_hover_text("Phase 4");
        });
    });
    egui::ScrollArea::vertical().id_salt("unstaged").max_height(half).auto_shrink([false, false]).show(ui, |ui| {
        for f in s.conflicted.iter().chain(s.unstaged.iter()) {
            let target = DiffTarget::WorkdirUnstaged(f.path.clone());
            let selected = app.selected_file.as_ref() == Some(&target);
            if file_row(ui, f, selected, &app.theme).clicked() {
                clicked = Some(target);
            }
        }
        if s.unstaged.is_empty() && s.conflicted.is_empty() {
            ui.weak("nothing unstaged");
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(format!("Staged ({})", s.staged.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled(false, egui::Button::new("Unstage all")).on_disabled_hover_text("Phase 4");
        });
    });
    egui::ScrollArea::vertical().id_salt("staged").max_height(half).auto_shrink([false, false]).show(ui, |ui| {
        for f in &s.staged {
            let target = DiffTarget::Staged(f.path.clone());
            let selected = app.selected_file.as_ref() == Some(&target);
            if file_row(ui, f, selected, &app.theme).clicked() {
                clicked = Some(target);
            }
        }
        if s.staged.is_empty() {
            ui.weak("nothing staged");
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.weak(format!("{} <{}>", s.user_name, s.user_email));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled(false, egui::Button::new("Commit")).on_disabled_hover_text("Phase 4");
            ui.checkbox(&mut app.amend, "amend");
        });
    });
    ui.add(egui::TextEdit::multiline(&mut app.commit_msg).hint_text("Commit message").desired_rows(2).desired_width(f32::INFINITY));
    if let Some(t) = clicked {
        app.focus = Pane::Detail;
        app.select_file(Some(t));
    }
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
}
