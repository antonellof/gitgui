//! gitgui logo: simple git branch tree beside the sidebar title.

use egui::{pos2, Painter, Rect, Stroke, Ui};

use crate::ui::theme::Theme;

/// Allocate and draw the logo beside the sidebar title.
pub fn show(ui: &mut Ui, theme: &Theme) {
    let h = ui.text_style_height(&egui::TextStyle::Heading);
    let size = egui::vec2(h, h);
    let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        draw(&ui.painter_at(rect), rect, theme);
    }
}

/// Draw the logo into `rect`.
pub fn draw(painter: &Painter, rect: Rect, theme: &Theme) {
    let pad = rect.width().min(rect.height()) * 0.1;
    let inner = rect.shrink(pad);
    let w = inner.width();
    let h = inner.height();
    let r = w.min(h) * 0.13;
    let color = theme.branch_pill;
    let stroke = Stroke::new((r * 0.45).max(1.0), color);

    let trunk_x = inner.min.x + w * 0.32;
    let top = pos2(trunk_x, inner.min.y + h * 0.2);
    let mid = pos2(trunk_x, inner.min.y + h * 0.55);
    let branch = pos2(inner.min.x + w * 0.82, inner.min.y + h * 0.55);

    painter.line_segment([top, mid], stroke);
    painter.line_segment([mid, branch], stroke);
    for c in [top, mid, branch] {
        painter.circle_filled(c, r, color);
        painter.circle_stroke(c, r, Stroke::new(1.0, theme.background));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Vec2;

    #[test]
    fn logo_nodes_fit_inside_rect() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(20.0, 20.0));
        let pad = rect.width().min(rect.height()) * 0.1;
        let inner = rect.shrink(pad);
        let w = inner.width();
        let h = inner.height();
        let r = w.min(h) * 0.13;
        let trunk_x = inner.min.x + w * 0.32;
        let nodes = [
            pos2(trunk_x, inner.min.y + h * 0.2),
            pos2(trunk_x, inner.min.y + h * 0.55),
            pos2(inner.min.x + w * 0.82, inner.min.y + h * 0.55),
        ];
        for c in nodes {
            assert!(inner.contains(c));
            assert!(c.x - r >= inner.min.x - 0.01);
            assert!(c.x + r <= inner.max.x + 0.01);
            assert!(c.y - r >= inner.min.y - 0.01);
            assert!(c.y + r <= inner.max.y + 0.01);
        }
    }
}
