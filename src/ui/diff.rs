//! Diff viewer: monospace lines with old/new numbers and colored backgrounds.

use egui::{pos2, vec2, Rect, Sense};

use crate::git::ops::Command;
use crate::git::repo::{DiffTarget, DiffText};
use crate::ui::app::App;

enum Row<'a> {
    Hunk(&'a str),
    Line(&'a crate::git::repo::DiffLine),
}

/// Commands the diff view wants sent (hunk staging arrives in Phase 4).
pub fn take_pending(_app: &mut App) -> Option<Command> {
    None
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let theme = app.theme.clone();
    ui.horizontal(|ui| {
        match &app.selected_file {
            Some(t) => {
                let kind = match t {
                    DiffTarget::WorkdirUnstaged(_) => "unstaged",
                    DiffTarget::Staged(_) => "staged",
                    DiffTarget::Commit(..) => "commit",
                };
                ui.monospace(t.path());
                ui.weak(kind);
            }
            None => {
                ui.weak("no file selected");
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut app.wrap, "wrap");
        });
    });
    ui.separator();
    let Some(d) = app.diff.as_ref() else {
        if app.diff_loading {
            ui.weak("loading diff");
        }
        return;
    };
    if d.too_large {
        ui.colored_label(theme.error, "file too large to diff (over 2 MB)");
    }
    if d.binary {
        ui.weak("binary file");
    }
    if d.hunks.is_empty() && !d.binary && !d.too_large {
        ui.weak("no changes");
        return;
    }
    let rows = flatten(d);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let max_no = rows
        .iter()
        .filter_map(|r| match r {
            Row::Line(l) => Some(l.old_no.unwrap_or(0).max(l.new_no.unwrap_or(0))),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let digits = (max_no.max(1) as f32).log10().floor() as usize + 1;
    let gutter = char_w * (digits as f32 * 2.0 + 4.0);
    let longest = rows
        .iter()
        .map(|r| match r {
            Row::Line(l) => l.text.chars().count(),
            Row::Hunk(h) => h.chars().count(),
        })
        .max()
        .unwrap_or(0);
    let wrap = app.wrap;
    let mut area = egui::ScrollArea::vertical().id_salt("diff_scroll").auto_shrink([false, false]);
    if !wrap {
        area = egui::ScrollArea::both().id_salt("diff_scroll").auto_shrink([false, false]);
    }
    let text_color = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    area.show_rows(ui, row_h, rows.len(), |ui, range| {
        let view_w = ui.available_width();
        let content_w = if wrap { view_w } else { view_w.max(gutter + longest as f32 * char_w + 16.0) };
        for i in range {
            let (rect, _resp) = ui.allocate_exact_size(vec2(content_w, row_h), Sense::hover());
            let p = ui.painter();
            match &rows[i] {
                Row::Hunk(header) => {
                    p.rect_filled(rect, 0.0, theme.hunk_bg);
                    p.text(pos2(rect.min.x + 6.0, rect.center().y), egui::Align2::LEFT_CENTER, *header, font.clone(), theme.hunk_fg);
                }
                Row::Line(l) => {
                    let (bg, fg) = match l.origin {
                        '+' => (Some(theme.add_bg), theme.add_fg),
                        '-' => (Some(theme.del_bg), theme.del_fg),
                        _ => (None, text_color),
                    };
                    if let Some(bg) = bg {
                        p.rect_filled(rect, 0.0, bg);
                    }
                    let old = l.old_no.map(|n| n.to_string()).unwrap_or_default();
                    let new = l.new_no.map(|n| n.to_string()).unwrap_or_default();
                    let no_x = rect.min.x + 4.0;
                    p.text(pos2(no_x + char_w * digits as f32, rect.center().y), egui::Align2::RIGHT_CENTER, old, font.clone(), theme.line_no);
                    p.text(
                        pos2(no_x + char_w * (digits as f32 * 2.0 + 1.0), rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        new,
                        font.clone(),
                        theme.line_no,
                    );
                    let sign_x = rect.min.x + gutter - char_w * 1.5;
                    p.text(pos2(sign_x, rect.center().y), egui::Align2::CENTER_CENTER, l.origin.to_string(), font.clone(), if l.origin == ' ' { text_color } else { strong });
                    let text_rect = Rect::from_min_max(pos2(rect.min.x + gutter, rect.min.y), rect.max);
                    let galley = if wrap {
                        p.layout(l.text.clone(), font.clone(), fg, text_rect.width())
                    } else {
                        p.layout_no_wrap(l.text.clone(), font.clone(), fg)
                    };
                    p.with_clip_rect(text_rect).galley(pos2(text_rect.min.x, rect.center().y - galley.size().y / 2.0), galley, fg);
                }
            }
        }
    });
}

fn flatten(d: &DiffText) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for h in &d.hunks {
        rows.push(Row::Hunk(h.header.as_str()));
        for l in &h.lines {
            rows.push(Row::Line(l));
        }
    }
    rows
}
