//! Commit list with the graph column.

use egui::{pos2, vec2, Color32, Pos2, Rect, Sense, Stroke};

use crate::git::graph::{EdgeKind, RowLayout};
use crate::ui::app::{age, App, Pane, Selection};
use crate::ui::menus::{self, CommitMenu};

pub const ROW_HEIGHT: f32 = 22.0;
const LANE_WIDTH: f32 = 14.0;
const MAX_LANES: usize = 12;
const NODE_RADIUS: f32 = 3.5;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let focused = app.focus == Pane::Log;
    ui.horizontal(|ui| {
        ui.strong("Commits");
        if app.snapshot.truncated {
            ui.weak(format!("(first {})", app.snapshot.commits.len()));
            if ui.small_button("load more").clicked() {
                app.pending.push(crate::git::ops::Command::LoadMore(
                    app.snapshot.commits.len() + 2000,
                ));
            }
        }
        if app.filter_active || !app.filter.is_empty() {
            let edit = egui::TextEdit::singleline(&mut app.filter)
                .hint_text("filter summary, author, hash")
                .desired_width(260.0);
            let resp = ui.add(edit);
            if app.filter_focus_requested {
                resp.request_focus();
                app.filter_focus_requested = false;
            }
            if resp.changed() {
                app.rebuild_filter();
            }
            if ui.small_button("x").clicked() {
                app.filter.clear();
                app.filter_active = false;
                app.rebuild_filter();
            }
        } else {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak("/ to filter");
            });
        }
    });
    ui.separator();

    let rows = app.log_rows();
    let lanes = app.snapshot.graph.max_lanes.clamp(1, MAX_LANES);
    let graph_w = lanes as f32 * LANE_WIDTH + 6.0;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let theme = app.theme.clone();

    let mut new_selection = None;
    let mut menu_pick: Option<(usize, CommitMenu)> = None;
    let mut scroll_target: Option<usize> = None;
    if app.scroll_to_selection {
        scroll_target = rows.iter().position(|r| *r == app.selection);
        app.scroll_to_selection = false;
    }

    let area = egui::ScrollArea::vertical()
        .id_salt("log_scroll")
        .auto_shrink([false, false]);
    let total = rows.len();
    area.show_rows(ui, ROW_HEIGHT, total, |ui, range| {
        let width = ui.available_width();
        for i in range {
            let sel = rows[i];
            let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::click());
            let selected = sel == app.selection;
            if selected {
                let color = if focused {
                    theme.selection
                } else {
                    theme.selection_inactive
                };
                ui.painter().rect_filled(rect, 3.0, color);
            } else if resp.hovered() {
                ui.painter()
                    .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            if resp.clicked() {
                new_selection = Some(sel);
            }
            if let Selection::Commit(ci) = sel {
                if resp.secondary_clicked() {
                    new_selection = Some(sel);
                }
                resp.context_menu(|ui| {
                    if let Some(a) = menus::commit_menu(ui, app, ci) {
                        menu_pick = Some((ci, a));
                    }
                });
            }
            if selected && scroll_target == Some(i) {
                ui.scroll_to_rect(rect, Some(egui::Align::Center));
            }
            let painter = ui.painter();
            let graph_rect = Rect::from_min_size(rect.min, vec2(graph_w, ROW_HEIGHT));
            let text_x = rect.min.x + graph_w + 4.0;
            match sel {
                Selection::WorkingTree => {
                    let center = pos2(graph_rect.min.x + 3.0 + LANE_WIDTH / 2.0, rect.center().y);
                    painter.circle_stroke(
                        center,
                        NODE_RADIUS,
                        Stroke::new(1.5, theme.graph_color(0)),
                    );
                    let s = &app.snapshot;
                    let text = if s.commits.is_empty() && !s.is_dirty() {
                        "Working tree (empty repository)".to_owned()
                    } else {
                        format!(
                            "Working tree: {} unstaged, {} staged{}{}",
                            s.unstaged.len(),
                            s.staged.len(),
                            if s.conflicted.is_empty() {
                                String::new()
                            } else {
                                format!(", {} conflicted", s.conflicted.len())
                            },
                            if s.state == crate::git::repo::RepoState::Clean {
                                String::new()
                            } else {
                                format!(" ({} in progress)", s.state.label())
                            }
                        )
                    };
                    painter.text(
                        pos2(text_x, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        text,
                        egui::TextStyle::Body.resolve(ui.style()),
                        ui.visuals().strong_text_color(),
                    );
                }
                Selection::Commit(ci) => {
                    let Some(c) = app.snapshot.commits.get(ci) else { continue };
                    if let Some(layout) = app.snapshot.graph.rows.get(ci) {
                        let next = app.snapshot.graph.rows.get(ci + 1);
                        draw_graph_row(painter, graph_rect, layout, next, &theme, lanes);
                    }
                    let mut x = text_x;
                    let font = egui::TextStyle::Small.resolve(ui.style());
                    for r in &c.refs {
                        let galley =
                            painter.layout_no_wrap(r.name.clone(), font.clone(), Color32::WHITE);
                        let w = galley.size().x + 8.0;
                        let pill =
                            Rect::from_min_size(pos2(x, rect.center().y - 8.0), vec2(w, 16.0));
                        painter.rect_filled(pill, 4.0, theme.pill(r.kind));
                        painter.galley(
                            pos2(x + 4.0, rect.center().y - galley.size().y / 2.0),
                            galley,
                            Color32::WHITE,
                        );
                        x += w + 4.0;
                    }
                    let right_w = 150.0;
                    let summary_w = (rect.max.x - right_w - x).max(40.0);
                    let body = egui::TextStyle::Body.resolve(ui.style());
                    let color = ui.visuals().text_color();
                    let galley = painter.layout_no_wrap(c.summary.clone(), body.clone(), color);
                    let clip =
                        Rect::from_min_max(pos2(x, rect.min.y), pos2(x + summary_w, rect.max.y));
                    painter.with_clip_rect(clip).galley(
                        pos2(x, rect.center().y - galley.size().y / 2.0),
                        galley,
                        color,
                    );
                    let weak = ui.visuals().weak_text_color();
                    let a = painter.layout_no_wrap(c.author.clone(), font.clone(), weak);
                    let author_x = rect.max.x - right_w + 4.0;
                    let aclip = Rect::from_min_max(
                        pos2(author_x, rect.min.y),
                        pos2(rect.max.x - 40.0, rect.max.y),
                    );
                    painter.with_clip_rect(aclip).galley(
                        pos2(author_x, rect.center().y - a.size().y / 2.0),
                        a,
                        weak,
                    );
                    painter.text(
                        pos2(rect.max.x - 6.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        age(now, c.time),
                        font.clone(),
                        weak,
                    );
                }
            }
        }
    });
    if let Some(sel) = new_selection {
        app.focus = Pane::Log;
        app.select(sel);
    }
    if let Some((ci, action)) = menu_pick {
        let ctx = ui.ctx().clone();
        menus::apply_commit_menu(app, &ctx, ci, action);
    }
    if focused && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        app.focus = Pane::Detail;
    }
}

fn lane_x(rect: Rect, lane: usize) -> f32 {
    rect.min.x + 3.0 + LANE_WIDTH / 2.0 + lane as f32 * LANE_WIDTH
}

fn draw_graph_row(
    painter: &egui::Painter,
    rect: Rect,
    row: &RowLayout,
    next: Option<&RowLayout>,
    theme: &crate::ui::theme::Theme,
    max_lanes: usize,
) {
    let top = rect.min.y;
    let bottom = rect.max.y;
    let mid = rect.center().y;
    let visible = |lane: usize| lane < max_lanes;
    let stroke = |color: usize| Stroke::new(1.5, theme.graph_color(color));

    // Lanes passing straight through.
    for (lane, color) in &row.through {
        if visible(*lane) {
            let x = lane_x(rect, *lane);
            painter.line_segment([pos2(x, top), pos2(x, bottom)], stroke(*color));
        }
    }
    let cx = lane_x(rect, row.lane);
    // Line into the commit from above (unless this lane starts here).
    let starts_here = next.is_none() && row.through.is_empty() && false;
    if !starts_here {
        painter.line_segment([pos2(cx, top), pos2(cx, mid)], stroke(row.color));
    }
    // Continuation below to the first parent: the next row shows it as a
    // through lane or as its own commit, so we draw only our half.
    let continues = row
        .edges
        .iter()
        .all(|e| e.kind != EdgeKind::Merge || e.to_lane != row.lane)
        && has_parent_below(row, next);
    if continues {
        painter.line_segment([pos2(cx, mid), pos2(cx, bottom)], stroke(row.color));
    }
    for e in &row.edges {
        match e.kind {
            EdgeKind::Fork => {
                if visible(e.to_lane) {
                    let tx = lane_x(rect, e.to_lane);
                    let color = next
                        .and_then(|n| {
                            n.through
                                .iter()
                                .find(|(l, _)| *l == e.to_lane)
                                .map(|(_, c)| *c)
                        })
                        .unwrap_or(row.color);
                    curve(painter, pos2(cx, mid), pos2(tx, bottom), stroke(color));
                }
            }
            EdgeKind::Merge => {
                if visible(e.from_lane) {
                    let fx = lane_x(rect, e.from_lane);
                    let color = row
                        .through
                        .iter()
                        .find(|(l, _)| *l == e.from_lane)
                        .map(|(_, c)| *c)
                        .unwrap_or(row.color);
                    curve(painter, pos2(fx, top), pos2(cx, mid), stroke(color));
                }
            }
        }
    }
    let node = pos2(cx, mid);
    if row.is_merge {
        painter.circle_filled(node, NODE_RADIUS, theme.background);
        painter.circle_stroke(node, NODE_RADIUS, stroke(row.color));
    } else {
        painter.circle_filled(node, NODE_RADIUS, theme.graph_color(row.color));
    }
    if row.width > max_lanes {
        let fade = Rect::from_min_max(pos2(rect.max.x - 10.0, top), pos2(rect.max.x, bottom));
        painter.rect_filled(fade, 0.0, theme.background.gamma_multiply(0.8));
    }
}

fn has_parent_below(row: &RowLayout, next: Option<&RowLayout>) -> bool {
    match next {
        None => false,
        Some(n) => {
            n.lane == row.lane
                || n.through.iter().any(|(l, _)| *l == row.lane)
                || n.edges
                    .iter()
                    .any(|e| e.kind == EdgeKind::Merge && e.from_lane == row.lane)
        }
    }
}

/// Quarter-circle-ish curve between two points, drawn as a short polyline.
fn curve(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let n = 8;
    let ctrl = pos2(to.x, from.y);
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let a = from.lerp(ctrl, t);
        let b = ctrl.lerp(to, t);
        pts.push(a.lerp(b, t));
    }
    painter.add(egui::Shape::line(pts, stroke));
}
