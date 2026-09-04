//! A row with trailing widgets laid out from the right edge and leading
//! content that takes what is left. The leading side is clipped to its own
//! rect so the two never overlap in a narrow pane.

use egui::{Align, Layout, Ui, UiBuilder};

pub fn split<R>(
    ui: &mut Ui,
    trailing: impl FnOnce(&mut Ui),
    leading: impl FnOnce(&mut Ui) -> R,
) -> R {
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        trailing(ui);
        let left = ui.available_rect_before_wrap();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(left)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                leading(ui)
            },
        )
        .inner
    })
    .inner
}
