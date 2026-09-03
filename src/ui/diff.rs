//! Diff viewer: monospace lines with old/new numbers and colored backgrounds.

use egui::{pos2, vec2, Color32, FontId, Rect, Sense};

use crate::git::ops::Command;
use crate::git::repo::{DiffLine, DiffTarget, DiffText};
use crate::ui::app::App;
use crate::ui::theme::Theme;

enum Row<'a> {
    Hunk(usize, &'a str),
    Line(&'a DiffLine),
}

struct PaintCtx<'a> {
    theme: &'a Theme,
    font: FontId,
    row_h: f32,
    char_w: f32,
    digits: usize,
    gutter: f32,
    text_color: Color32,
    strong: Color32,
    hunk_action: Option<(String, bool)>,
    busy: bool,
}

thread_local! {
    static PENDING: std::cell::RefCell<Option<Command>> = const { std::cell::RefCell::new(None) };
}

/// Command produced by a hunk button click this frame, if any.
pub fn take_pending(_app: &mut App) -> Option<Command> {
    PENDING.with(|p| p.borrow_mut().take())
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
            Row::Hunk(_, h) => h.chars().count(),
        })
        .max()
        .unwrap_or(0);
    let wrap = app.wrap;
    let text_color = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    let hunk_action = match &d.target {
        DiffTarget::WorkdirUnstaged(p) => Some((p.to_owned(), true)),
        DiffTarget::Staged(p) => Some((p.to_owned(), false)),
        DiffTarget::Commit(..) => None,
    };
    let ctx = PaintCtx {
        theme: &theme,
        font: font.clone(),
        row_h,
        char_w,
        digits,
        gutter,
        text_color,
        strong,
        hunk_action,
        busy: app.busy > 0,
    };

    if wrap {
        egui::ScrollArea::vertical()
            .id_salt("diff_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let view_w = ui.available_width();
                for row in &rows {
                    paint_row(ui, &ctx, row, view_w, view_w, true);
                }
            });
    } else {
        let content_w = ui.available_width().max(gutter + longest as f32 * char_w + 16.0);
        egui::ScrollArea::both()
            .id_salt("diff_scroll")
            .auto_shrink([false, false])
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                let view_w = ui.available_width();
                for i in range {
                    paint_row(ui, &ctx, &rows[i], view_w, content_w, false);
                }
            });
    }
}

fn line_colors(origin: char, ctx: &PaintCtx) -> (Option<Color32>, Color32) {
    match origin {
        '+' => (Some(ctx.theme.add_bg), ctx.theme.add_fg),
        '-' => (Some(ctx.theme.del_bg), ctx.theme.del_fg),
        _ => (None, ctx.text_color),
    }
}

fn paint_row(
    ui: &mut egui::Ui,
    ctx: &PaintCtx,
    row: &Row<'_>,
    view_w: f32,
    content_w: f32,
    wrap: bool,
) {
    let p = ui.painter().clone();
    match row {
        Row::Hunk(hunk_index, header) => {
            let header_galley = if wrap {
                Some(p.layout(
                    (*header).to_owned(),
                    ctx.font.clone(),
                    ctx.theme.hunk_fg,
                    (view_w - 12.0).max(1.0),
                ))
            } else {
                None
            };
            let height = header_galley
                .as_ref()
                .map(|g| g.size().y.max(ctx.row_h))
                .unwrap_or(ctx.row_h);
            let (rect, _resp) = ui.allocate_exact_size(vec2(content_w, height), Sense::hover());
            p.rect_filled(rect, 0.0, ctx.theme.hunk_bg);
            if let Some(galley) = &header_galley {
                p.galley(
                    pos2(rect.min.x + 6.0, rect.min.y + 1.0),
                    galley.clone(),
                    ctx.theme.hunk_fg,
                );
            } else {
                p.text(
                    pos2(rect.min.x + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    *header,
                    ctx.font.clone(),
                    ctx.theme.hunk_fg,
                );
            }
            if let Some((path, stage)) = &ctx.hunk_action {
                let label = if *stage { "Stage hunk" } else { "Unstage hunk" };
                let bw = 100.0;
                let brect = Rect::from_min_size(
                    pos2(rect.min.x + view_w - bw - 8.0, rect.min.y + 1.0),
                    vec2(bw, ctx.row_h - 2.0),
                );
                let clicked = ui.put(brect, egui::Button::new(label).small()).clicked();
                if clicked && !ctx.busy {
                    let cmd = if *stage {
                        Command::StageHunk {
                            path: path.clone(),
                            hunk_index: *hunk_index,
                        }
                    } else {
                        Command::UnstageHunk {
                            path: path.clone(),
                            hunk_index: *hunk_index,
                        }
                    };
                    PENDING.with(|pending| *pending.borrow_mut() = Some(cmd));
                }
            }
        }
        Row::Line(l) => {
            let (bg, fg) = line_colors(l.origin, ctx);
            let text_w = (content_w - ctx.gutter).max(1.0);
            let galley = if wrap {
                p.layout(l.text.clone(), ctx.font.clone(), fg, text_w)
            } else {
                p.layout_no_wrap(l.text.clone(), ctx.font.clone(), fg)
            };
            let height = if wrap {
                galley.size().y.max(ctx.row_h)
            } else {
                ctx.row_h
            };
            let (rect, _resp) = ui.allocate_exact_size(vec2(content_w, height), Sense::hover());
            let text_rect =
                Rect::from_min_max(pos2(rect.min.x + ctx.gutter, rect.min.y), rect.max);
            if let Some(bg) = bg {
                p.rect_filled(rect, 0.0, bg);
            }
            paint_line_gutter(&p, rect, l, ctx, wrap);
            let galley_pos = if wrap {
                pos2(text_rect.min.x, text_rect.min.y + 1.0)
            } else {
                pos2(
                    text_rect.min.x,
                    rect.center().y - galley.size().y / 2.0,
                )
            };
            p.with_clip_rect(text_rect)
                .galley(galley_pos, galley, fg);
        }
    }
}

fn paint_line_gutter(p: &egui::Painter, rect: Rect, l: &DiffLine, ctx: &PaintCtx, wrap: bool) {
    let old = l.old_no.map(|n| n.to_string()).unwrap_or_default();
    let new = l.new_no.map(|n| n.to_string()).unwrap_or_default();
    let no_x = rect.min.x + 4.0;
    let line_y = if wrap {
        rect.min.y + 1.0 + ctx.row_h / 2.0
    } else {
        rect.center().y
    };
    p.text(
        pos2(no_x + ctx.char_w * ctx.digits as f32, line_y),
        egui::Align2::RIGHT_CENTER,
        old,
        ctx.font.clone(),
        ctx.theme.line_no,
    );
    p.text(
        pos2(
            no_x + ctx.char_w * (ctx.digits as f32 * 2.0 + 1.0),
            line_y,
        ),
        egui::Align2::RIGHT_CENTER,
        new,
        ctx.font.clone(),
        ctx.theme.line_no,
    );
    let sign_x = rect.min.x + ctx.gutter - ctx.char_w * 1.5;
    p.text(
        pos2(sign_x, line_y),
        egui::Align2::CENTER_CENTER,
        l.origin.to_string(),
        ctx.font.clone(),
        if l.origin == ' ' {
            ctx.text_color
        } else {
            ctx.strong
        },
    );
}

fn flatten(d: &DiffText) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for (i, h) in d.hunks.iter().enumerate() {
        rows.push(Row::Hunk(i, h.header.as_str()));
        for l in &h.lines {
            rows.push(Row::Line(l));
        }
    }
    rows
}
