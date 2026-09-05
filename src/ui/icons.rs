//! Small UI icons drawn with epaint.

use egui::{pos2, Color32, Painter, Rect, Stroke, Ui, Vec2};

/// Small chevron-down beside the current branch (dropdown hint).
pub fn chevron_down(ui: &mut Ui, color: Color32) -> egui::Response {
    let h = ui.spacing().interact_size.y * 0.55;
    let w = h * 1.1;
    let size = Vec2::new(w, h);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        draw_chevron_down(&ui.painter_at(rect), rect, color);
    }
    response
}

pub fn draw_chevron_down(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new((rect.height() * 0.14).max(1.0), color);
    let cx = rect.center().x;
    let top = rect.min.y + rect.height() * 0.28;
    let bot = rect.max.y - rect.height() * 0.28;
    let wing = rect.width() * 0.42;
    painter.line_segment([pos2(cx - wing, top), pos2(cx, bot)], stroke);
    painter.line_segment([pos2(cx + wing, top), pos2(cx, bot)], stroke);
}

/// Small right / down triangle for tree rows.
pub fn disclosure(ui: &mut Ui, open: bool, color: Color32) -> egui::Response {
    let h = ui.spacing().interact_size.y * 0.5;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(h, h), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let r = rect.shrink(h * 0.22);
        let pts = if open {
            vec![r.left_top(), r.right_top(), pos2(r.center().x, r.bottom())]
        } else {
            vec![r.left_top(), pos2(r.right(), r.center().y), r.left_bottom()]
        };
        ui.painter_at(rect)
            .add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    }
    response
}
