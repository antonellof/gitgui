//! Diff viewer: monospace lines with old/new numbers and colored backgrounds,
//! hunk buttons, line selection for partial staging, search, context and
//! whitespace controls.

use egui::{pos2, vec2, Color32, FontId, Rect, Sense};

use crate::git::actions::ConflictSide;
use crate::git::ops::Command;
use crate::git::repo::{DiffLine, DiffTarget, DiffText};
use crate::ui::app::{App, LineSel};
use crate::ui::theme::Theme;

enum Row<'a> {
    Hunk(usize, &'a str),
    /// (hunk index, line index within the hunk, line)
    Line(usize, usize, &'a DiffLine),
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
    /// (path, unstaged?) when hunk buttons apply.
    hunk_action: Option<(String, bool)>,
    busy: bool,
    sel: Option<LineSel>,
    query: String,
    current_match: Option<usize>,
}

/// What a row interaction asked for; applied after the paint loop.
enum RowAction {
    Click { hunk: usize, line: usize, shift: bool },
    DragStart { hunk: usize, line: usize },
    DragOver { hunk: usize, line: usize },
}

/// Width of the stage / unstage button drawn on the right of a hunk header.
const HUNK_BUTTON_W: f32 = 100.0;
const DISCARD_BUTTON_W: f32 = 96.0;

thread_local! {
    static PENDING: std::cell::RefCell<Option<Command>> = const { std::cell::RefCell::new(None) };
}

/// Command produced by a hunk button click this frame, if any.
pub fn take_pending(_app: &mut App) -> Option<Command> {
    PENDING.with(|p| p.borrow_mut().take())
}

fn flatten(d: &DiffText) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    for (i, h) in d.hunks.iter().enumerate() {
        rows.push(Row::Hunk(i, h.header.as_str()));
        for (j, l) in h.lines.iter().enumerate() {
            rows.push(Row::Line(i, j, l));
        }
    }
    rows
}

fn row_text<'a>(row: &Row<'a>) -> &'a str {
    match row {
        Row::Hunk(_, h) => h,
        Row::Line(_, _, l) => l.text.as_str(),
    }
}

/// Row indices whose text contains `query` (case-insensitive).
fn matches(rows: &[Row<'_>], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let q = query.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| row_text(r).to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// Number of rows matching the current search.
pub fn match_count(app: &App) -> usize {
    match app.diff.as_ref() {
        Some(d) => matches(&flatten(d), &app.diff_search).len(),
        None => 0,
    }
}

fn header(app: &mut App, ui: &mut egui::Ui) {
    let selected = app.selected_file.clone();
    let has_sel = app.has_line_selection();
    let sel_count = app
        .line_sel
        .map(|s| {
            let (a, b) = s.range();
            b - a + 1
        })
        .unwrap_or(0);
    let unstaged_target = matches!(selected, Some(DiffTarget::WorkdirUnstaged(_)));
    let conflicted = selected
        .as_ref()
        .is_some_and(|t| app.snapshot.conflicted.iter().any(|f| f.path == t.path()));
    let mut resolve: Option<ConflictSide> = None;
    let mut mark_resolved = false;
    let busy = app.busy > 0;
    let mut ctx_delta = 0;
    let mut toggle_ws = false;
    let mut toggle_search = false;
    let mut stage_lines = false;
    let mut unstage_lines = false;
    let mut discard_lines = false;
    let mut clear_sel = false;
    let context = app.diff_opts.context;
    let ignore_ws = app.diff_opts.ignore_whitespace;
    let search_active = app.diff_search_active;
    crate::ui::row::split(
        ui,
        |ui| {
            app.hide_button(ui, crate::ui::app::Pane::Detail);
            ui.checkbox(&mut app.wrap, "wrap");
            if ui
                .add(egui::Button::new("ws").small().selected(ignore_ws))
                .on_hover_text("Ignore whitespace (Ctrl+W)")
                .clicked()
            {
                toggle_ws = true;
            }
            if ui
                .small_button("+")
                .on_hover_text("More context ( } )")
                .clicked()
            {
                ctx_delta = 1;
            }
            ui.weak(format!("{context}"));
            if ui
                .small_button("-")
                .on_hover_text("Less context ( { )")
                .clicked()
            {
                ctx_delta = -1;
            }
            if ui
                .add(egui::Button::new("find").small().selected(search_active))
                .on_hover_text("Search in the diff (Ctrl+F)")
                .clicked()
            {
                toggle_search = true;
            }
        },
        |ui| match &selected {
            Some(t) => {
                let kind = match t {
                    DiffTarget::WorkdirUnstaged(_) => "unstaged",
                    DiffTarget::Staged(_) => "staged",
                    DiffTarget::Commit(..) => "commit",
                };
                ui.monospace(t.path());
                ui.weak(if conflicted { "conflicted" } else { kind });
                if conflicted {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Use ours").small())
                        .on_hover_text("keep the version of the branch you are on")
                        .clicked()
                    {
                        resolve = Some(ConflictSide::Ours);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Use theirs").small())
                        .on_hover_text("keep the incoming version")
                        .clicked()
                    {
                        resolve = Some(ConflictSide::Theirs);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Mark resolved").small())
                        .on_hover_text("stage the file as it is, markers and all")
                        .clicked()
                    {
                        mark_resolved = true;
                    }
                }
                if has_sel && !conflicted {
                    let label = |verb: &str| {
                        format!(
                            "{verb} {sel_count} line{}",
                            if sel_count == 1 { "" } else { "s" }
                        )
                    };
                    if unstaged_target {
                        if ui
                            .add_enabled(!busy, egui::Button::new(label("Stage")).small())
                            .on_hover_text("s")
                            .clicked()
                        {
                            stage_lines = true;
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new(label("Discard")).small())
                            .on_hover_text("d, asks for confirmation")
                            .clicked()
                        {
                            discard_lines = true;
                        }
                    } else if ui
                        .add_enabled(!busy, egui::Button::new(label("Unstage")).small())
                        .on_hover_text("u")
                        .clicked()
                    {
                        unstage_lines = true;
                    }
                    if ui.small_button("x").on_hover_text("Clear selection (Escape)").clicked() {
                        clear_sel = true;
                    }
                }
            }
            None => {
                ui.weak("no file selected");
            }
        },
    );
    if ctx_delta != 0 {
        app.change_diff_context(ctx_delta);
    }
    if toggle_ws {
        app.toggle_whitespace();
    }
    if toggle_search {
        if app.diff_search_active {
            app.close_diff_search();
        } else {
            app.open_diff_search();
        }
    }
    if stage_lines {
        app.stage_selected_lines();
    }
    if unstage_lines {
        app.unstage_selected_lines();
    }
    if discard_lines {
        app.discard_selected_lines();
    }
    if clear_sel {
        app.line_sel = None;
    }
    if let Some(side) = resolve {
        app.resolve_selected(side);
    }
    if mark_resolved {
        app.stage_selected();
    }
}

fn search_bar(app: &mut App, ui: &mut egui::Ui, total: usize) {
    let mut next = 0i32;
    let mut close = false;
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.diff_search)
                .hint_text("search in diff")
                .desired_width(220.0),
        );
        if app.diff_search_focus {
            resp.request_focus();
            app.diff_search_focus = false;
        }
        if resp.changed() {
            app.diff_match = 0;
            app.diff_jump = true;
        }
        if resp.has_focus() {
            let (enter, shift) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.shift));
            if enter {
                next = if shift { -1 } else { 1 };
                resp.request_focus();
            }
        }
        if total > 0 {
            ui.weak(format!("{} of {total}", app.diff_match.min(total.saturating_sub(1)) + 1));
        } else if !app.diff_search.is_empty() {
            ui.weak("no matches");
        }
        if ui.small_button("prev").on_hover_text("Shift+N").clicked() {
            next = -1;
        }
        if ui.small_button("next").on_hover_text("n").clicked() {
            next = 1;
        }
        if ui.small_button("close").on_hover_text("Escape").clicked() {
            close = true;
        }
    });
    if next != 0 {
        app.diff_next_match(next);
    }
    if close {
        app.close_diff_search();
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let theme = app.theme.clone();
    header(app, ui);
    let total_matches = match_count(app);
    if app.diff_search_active {
        search_bar(app, ui, total_matches);
    }
    ui.separator();
    if app.diff.is_none() {
        if app.diff_loading {
            ui.weak("loading diff");
        }
        return;
    }
    let d = app.diff.clone().expect("checked above");
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
    let rows = flatten(&d);
    let match_rows = matches(&rows, &app.diff_search);
    if !match_rows.is_empty() && app.diff_match >= match_rows.len() {
        app.diff_match = match_rows.len() - 1;
    }
    let current_match = match_rows.get(app.diff_match).copied();
    let jump = std::mem::take(&mut app.diff_jump);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let max_no = rows
        .iter()
        .filter_map(|r| match r {
            Row::Line(_, _, l) => Some(l.old_no.unwrap_or(0).max(l.new_no.unwrap_or(0))),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let digits = (max_no.max(1) as f32).log10().floor() as usize + 1;
    let gutter = char_w * (digits as f32 * 2.0 + 4.0);
    let longest = rows.iter().map(|r| row_text(r).chars().count()).max().unwrap_or(0);
    let wrap = app.wrap;
    let text_color = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    let hunk_action = match &d.target {
        _ if d.status == crate::git::repo::FileKind::Conflicted => None,
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
        sel: app.line_sel,
        query: app.diff_search.to_lowercase(),
        current_match,
    };
    let mut actions: Vec<RowAction> = Vec::new();
    let pointer_down = ui.input(|i| i.pointer.primary_down());
    if !pointer_down {
        app.line_drag = false;
    }
    let dragging = app.line_drag;

    if wrap {
        egui::ScrollArea::vertical()
            .id_salt("diff_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let view_w = ui.available_width();
                for (i, row) in rows.iter().enumerate() {
                    let rect = paint_row(ui, &ctx, i, row, view_w, view_w, true, dragging, &mut actions);
                    if jump && Some(i) == current_match {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }
                }
            });
    } else {
        let content_w = ui.available_width().max(gutter + longest as f32 * char_w + 16.0);
        let mut area = egui::ScrollArea::both()
            .id_salt("diff_scroll")
            .auto_shrink([false, false]);
        if jump {
            if let Some(m) = current_match {
                let offset = (m as f32 * row_h - ui.available_height() * 0.4).max(0.0);
                area = area.vertical_scroll_offset(offset);
            }
        }
        area.show_rows(ui, row_h, rows.len(), |ui, range| {
            let view_w = ui.available_width();
            for i in range {
                paint_row(ui, &ctx, i, &rows[i], view_w, content_w, false, dragging, &mut actions);
            }
        });
    }
    for a in actions {
        match a {
            RowAction::Click { hunk, line, shift } => {
                app.line_sel = match app.line_sel {
                    Some(mut s) if shift && s.hunk == hunk => {
                        s.end = line;
                        Some(s)
                    }
                    Some(s) if s.hunk == hunk && s.anchor == line && s.end == line => None,
                    _ => Some(LineSel {
                        hunk,
                        anchor: line,
                        end: line,
                    }),
                };
            }
            RowAction::DragStart { hunk, line } => {
                app.line_drag = true;
                app.line_sel = Some(LineSel {
                    hunk,
                    anchor: line,
                    end: line,
                });
            }
            RowAction::DragOver { hunk, line } => {
                if let Some(s) = app.line_sel.as_mut() {
                    if s.hunk == hunk {
                        s.end = line;
                    }
                }
            }
        }
    }
}

fn line_colors(origin: char, ctx: &PaintCtx) -> (Option<Color32>, Color32) {
    match origin {
        '+' => (Some(ctx.theme.add_bg), ctx.theme.add_fg),
        '-' => (Some(ctx.theme.del_bg), ctx.theme.del_fg),
        _ => (None, ctx.text_color),
    }
}

/// Paint highlights behind every occurrence of the query in `text`.
fn paint_matches(
    p: &egui::Painter,
    ctx: &PaintCtx,
    text: &str,
    origin_x: f32,
    rect: Rect,
    current: bool,
) {
    if ctx.query.is_empty() {
        return;
    }
    let lower = text.to_lowercase();
    let qlen = ctx.query.chars().count();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(&ctx.query) {
        let byte = from + pos;
        let col = lower[..byte].chars().count();
        let x0 = origin_x + col as f32 * ctx.char_w;
        let x1 = x0 + qlen as f32 * ctx.char_w;
        let color = if current {
            ctx.theme.tag_pill
        } else {
            ctx.theme.tag_pill.gamma_multiply(0.45)
        };
        p.rect_filled(
            Rect::from_min_max(pos2(x0, rect.min.y + 1.0), pos2(x1, rect.max.y - 1.0)),
            2.0,
            color,
        );
        from = byte + ctx.query.len();
        if from >= lower.len() {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    ui: &mut egui::Ui,
    ctx: &PaintCtx,
    row_index: usize,
    row: &Row<'_>,
    view_w: f32,
    content_w: f32,
    wrap: bool,
    dragging: bool,
    actions: &mut Vec<RowAction>,
) -> Rect {
    let p = ui.painter().clone();
    let is_current = ctx.current_match == Some(row_index);
    match row {
        Row::Hunk(hunk_index, header) => {
            let buttons_w = match &ctx.hunk_action {
                Some((_, true)) => HUNK_BUTTON_W + DISCARD_BUTTON_W + 8.0,
                Some((_, false)) => HUNK_BUTTON_W,
                None => 0.0,
            };
            let header_galley = if wrap {
                Some(p.layout(
                    (*header).to_owned(),
                    ctx.font.clone(),
                    ctx.theme.hunk_fg,
                    (view_w - buttons_w - 20.0).max(1.0),
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
            let text_right = rect.min.x + view_w - buttons_w - 14.0;
            let pc = p.with_clip_rect(
                Rect::from_min_max(rect.min, pos2(text_right, rect.max.y)).intersect(p.clip_rect()),
            );
            paint_matches(&pc, ctx, header, rect.min.x + 6.0, rect, is_current);
            if let Some(galley) = &header_galley {
                pc.galley(
                    pos2(rect.min.x + 6.0, rect.min.y + 1.0),
                    galley.clone(),
                    ctx.theme.hunk_fg,
                );
            } else {
                pc.text(
                    pos2(rect.min.x + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    *header,
                    ctx.font.clone(),
                    ctx.theme.hunk_fg,
                );
            }
            if let Some((path, unstaged)) = &ctx.hunk_action {
                let label = if *unstaged { "Stage hunk" } else { "Unstage hunk" };
                let brect = Rect::from_min_size(
                    pos2(rect.min.x + view_w - HUNK_BUTTON_W - 8.0, rect.min.y + 1.0),
                    vec2(HUNK_BUTTON_W, ctx.row_h - 2.0),
                );
                let clicked = ui.put(brect, egui::Button::new(label).small()).clicked();
                if clicked && !ctx.busy {
                    let cmd = if *unstaged {
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
                if *unstaged {
                    let drect = Rect::from_min_size(
                        pos2(brect.min.x - DISCARD_BUTTON_W - 8.0, rect.min.y + 1.0),
                        vec2(DISCARD_BUTTON_W, ctx.row_h - 2.0),
                    );
                    let clicked = ui
                        .put(drect, egui::Button::new("Discard hunk").small())
                        .on_hover_text("Undo this hunk in the working tree. Cannot be undone.")
                        .clicked();
                    if clicked && !ctx.busy {
                        PENDING.with(|pending| {
                            *pending.borrow_mut() = Some(Command::DiscardHunk {
                                path: path.clone(),
                                hunk_index: *hunk_index,
                            })
                        });
                    }
                }
            }
            rect
        }
        Row::Line(hunk, line, l) => {
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
            let selectable = ctx.hunk_action.is_some();
            let sense = if selectable {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(content_w, height), sense);
            if selectable {
                let shift = ui.input(|i| i.modifiers.shift);
                if resp.drag_started() {
                    actions.push(RowAction::DragStart {
                        hunk: *hunk,
                        line: *line,
                    });
                } else if resp.clicked() {
                    actions.push(RowAction::Click {
                        hunk: *hunk,
                        line: *line,
                        shift,
                    });
                } else if dragging && ui.rect_contains_pointer(rect) {
                    actions.push(RowAction::DragOver {
                        hunk: *hunk,
                        line: *line,
                    });
                }
            }
            let text_rect =
                Rect::from_min_max(pos2(rect.min.x + ctx.gutter, rect.min.y), rect.max);
            if let Some(bg) = bg {
                p.rect_filled(rect, 0.0, bg);
            }
            let selected = ctx.sel.is_some_and(|s| s.contains(*hunk, *line));
            if selected {
                let c = ctx.theme.selection;
                p.rect_filled(
                    rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 120),
                );
                p.rect_stroke(
                    Rect::from_min_max(pos2(rect.min.x + 1.0, rect.min.y), pos2(rect.min.x + 4.0, rect.max.y)),
                    0.0,
                    egui::Stroke::new(3.0, ctx.theme.hunk_fg),
                    egui::StrokeKind::Inside,
                );
            } else if selectable && resp.hovered() {
                p.rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill.gamma_multiply(0.5));
            }
            paint_line_gutter(&p, rect, l, ctx, wrap);
            let galley_pos = if wrap {
                pos2(text_rect.min.x, text_rect.min.y + 1.0)
            } else {
                pos2(text_rect.min.x, rect.center().y - galley.size().y / 2.0)
            };
            let pc = p.with_clip_rect(text_rect);
            if !wrap {
                paint_matches(&pc, ctx, &l.text, text_rect.min.x, rect, is_current);
            } else if is_current {
                pc.rect_filled(text_rect, 0.0, ctx.theme.tag_pill.gamma_multiply(0.35));
            }
            pc.galley(galley_pos, galley, fg);
            rect
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo::{FileKind, Hunk};

    fn line(origin: char, text: &str) -> DiffLine {
        DiffLine {
            origin,
            old_no: None,
            new_no: None,
            text: text.into(),
            no_newline: false,
        }
    }

    #[test]
    fn search_matches_rows_case_insensitively() {
        let d = DiffText {
            target: DiffTarget::Staged("f".into()),
            binary: false,
            too_large: false,
            status: FileKind::Modified,
            hunks: vec![Hunk {
                header: "@@ -1 +1 @@ fn Main".into(),
                lines: vec![line(' ', "let x = 1;"), line('+', "MAIN loop"), line('-', "old")],
            }],
        };
        let rows = flatten(&d);
        assert_eq!(rows.len(), 4);
        assert_eq!(matches(&rows, "main"), vec![0, 2]);
        assert!(matches(&rows, "").is_empty());
        assert_eq!(matches(&rows, "OLD"), vec![3]);
    }

    #[test]
    fn line_selection_ranges() {
        let s = LineSel {
            hunk: 1,
            anchor: 5,
            end: 2,
        };
        assert_eq!(s.range(), (2, 5));
        assert_eq!(s.lines(), vec![2, 3, 4, 5]);
        assert!(s.contains(1, 3));
        assert!(!s.contains(0, 3));
        assert!(!s.contains(1, 6));
    }
}
