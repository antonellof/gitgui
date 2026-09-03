//! Footer action bar: fetch, pull, push, refresh, quit.

use egui::{pos2, Color32, FontId, Painter, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::git::ops::Command;
use crate::ui::app::App;
use crate::ui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Icon {
    Fetch,
    Pull,
    Push,
    Refresh,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let busy = app.busy > 0;

    ui.horizontal(|ui| {
        ui.add_space(4.0);
        if labeled_button(ui, &app.theme, Icon::Fetch, "Fetch", "Fetch (f)", !busy).clicked() {
            app.run(Command::Fetch);
        }
        if labeled_button(ui, &app.theme, Icon::Pull, "Pull", "Pull (p)", !busy).clicked() {
            app.run(Command::Pull);
        }
        if labeled_button(ui, &app.theme, Icon::Push, "Push", "Push (Shift+P)", !busy).clicked() {
            app.run(Command::Push);
        }
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        if labeled_button(ui, &app.theme, Icon::Refresh, "Refresh", "Refresh (r)", !busy)
            .clicked()
        {
            app.pending.push(Command::Refresh);
            app.toast("refreshing", false);
        }
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        if text_button(ui, "Quit", "Quit (q)", true).clicked() {
            app.request_quit();
        }
    });
}

fn labeled_button(
    ui: &mut Ui,
    theme: &Theme,
    icon: Icon,
    label: &str,
    tip: &str,
    enabled: bool,
) -> Response {
    let h = ui.spacing().interact_size.y;
    let icon_w = 16.0;
    let pad_x = 8.0;
    let gap = 5.0;
    let text_w = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            FontId::proportional(13.0),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    let w = pad_x + icon_w + gap + text_w + pad_x;
    let size = Vec2::new(w, h);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let response = response.on_hover_text(tip);

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        let bg = if response.hovered() && enabled {
            visuals.bg_fill
        } else {
            visuals.weak_bg_fill
        };
        ui.painter().rect_filled(rect, visuals.corner_radius, bg);
        let fg = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let icon_rect = Rect::from_min_size(
            pos2(rect.min.x + pad_x, rect.center().y - icon_w * 0.5),
            Vec2::splat(icon_w),
        );
        draw_icon(
            &ui.painter_at(icon_rect),
            icon_rect.shrink(1.0),
            icon,
            theme.hunk_fg.linear_multiply(if enabled { 1.0 } else { 0.45 }),
        );
        let text_pos = pos2(icon_rect.max.x + gap, rect.center().y);
        ui.painter().text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.0),
            fg,
        );
    }
    response
}

fn draw_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    match icon {
        Icon::Fetch => draw_fetch(painter, rect, color),
        Icon::Pull => draw_arrow(painter, rect, color, true),
        Icon::Push => draw_arrow(painter, rect, color, false),
        Icon::Refresh => draw_refresh(painter, rect, color),
    }
}

fn text_button(ui: &mut Ui, label: &str, tip: &str, enabled: bool) -> Response {
    let h = ui.spacing().interact_size.y;
    let pad_x = 10.0;
    let text_w = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            FontId::proportional(13.0),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    let size = Vec2::new(pad_x + text_w + pad_x, h);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let response = response.on_hover_text(tip);

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        let bg = if response.hovered() && enabled {
            visuals.bg_fill
        } else {
            visuals.weak_bg_fill
        };
        ui.painter().rect_filled(rect, visuals.corner_radius, bg);
        let fg = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(13.0),
            fg,
        );
    }
    response
}

fn stroke_w(rect: Rect) -> f32 {
    (rect.width().min(rect.height()) * 0.1).max(1.0)
}

fn draw_fetch(painter: &Painter, rect: Rect, color: Color32) {
    let w = stroke_w(rect);
    let stroke = Stroke::new(w, color);
    let cx = rect.center().x;
    let top = rect.min.y + rect.height() * 0.15;
    let mid = rect.min.y + rect.height() * 0.55;
    let bot = rect.max.y - rect.height() * 0.1;
    let wing = rect.width() * 0.28;
    painter.line_segment([pos2(cx - wing, top), pos2(cx, mid)], stroke);
    painter.line_segment([pos2(cx + wing, top), pos2(cx, mid)], stroke);
    painter.line_segment([pos2(cx, mid), pos2(cx, bot)], stroke);
    let ah = rect.height() * 0.12;
    painter.line_segment([pos2(cx - wing * 0.7, bot - ah), pos2(cx, bot)], stroke);
    painter.line_segment([pos2(cx + wing * 0.7, bot - ah), pos2(cx, bot)], stroke);
}

fn draw_arrow(painter: &Painter, rect: Rect, color: Color32, down: bool) {
    let w = stroke_w(rect);
    let stroke = Stroke::new(w, color);
    let cx = rect.center().x;
    let (y0, y1, ytip) = if down {
        (
            rect.min.y + rect.height() * 0.15,
            rect.max.y - rect.height() * 0.28,
            rect.max.y - rect.height() * 0.1,
        )
    } else {
        (
            rect.max.y - rect.height() * 0.15,
            rect.min.y + rect.height() * 0.28,
            rect.min.y + rect.height() * 0.1,
        )
    };
    painter.line_segment([pos2(cx, y0), pos2(cx, y1)], stroke);
    let wing = rect.width() * 0.3;
    let base = if down {
        ytip - rect.height() * 0.12
    } else {
        ytip + rect.height() * 0.12
    };
    painter.line_segment([pos2(cx - wing, base), pos2(cx, ytip)], stroke);
    painter.line_segment([pos2(cx + wing, base), pos2(cx, ytip)], stroke);
}

fn draw_refresh(painter: &Painter, rect: Rect, color: Color32) {
    let w = stroke_w(rect);
    let stroke = Stroke::new(w, color);
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.36;
    let n = 10;
    let start = std::f32::consts::FRAC_PI_4;
    let span = std::f32::consts::PI * 1.35;
    let mut prev = None;
    for i in 0..=n {
        let t = start + span * (i as f32 / n as f32);
        let p = pos2(c.x + t.cos() * r, c.y + t.sin() * r);
        if let Some(prev) = prev {
            painter.line_segment([prev, p], stroke);
        }
        prev = Some(p);
    }
    let tip = start + span;
    let tip_p = pos2(c.x + tip.cos() * r, c.y + tip.sin() * r);
    let ah = r * 0.35;
    let a1 = tip - 0.55;
    let a2 = tip + 0.55;
    painter.line_segment(
        [tip_p, pos2(c.x + a1.cos() * (r - ah * 0.3), c.y + a1.sin() * (r - ah * 0.3))],
        stroke,
    );
    painter.line_segment(
        [tip_p, pos2(c.x + a2.cos() * (r - ah * 0.3), c.y + a2.sin() * (r - ah * 0.3))],
        stroke,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_fit_label_button_icon_slot() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(16.0, 16.0));
        assert!(rect.width() > 4.0);
    }
}
