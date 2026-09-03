//! Branches, remotes, tags, stashes.

use crate::ui::app::{App, Pane, Selection};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let focused = app.focus == Pane::Sidebar;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.heading("gitgui");
        if focused {
            ui.weak("*");
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().id_salt("sidebar_scroll").show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let snapshot = app.snapshot.clone();
        let mut clicked: Option<(String, git2::Oid)> = None;

        egui::CollapsingHeader::new("Local").default_open(true).show(ui, |ui| {
            for b in snapshot.branches.iter().filter(|b| !b.is_remote) {
                let label = if b.is_head { format!("* {}", b.name) } else { format!("  {}", b.name) };
                let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
                let mut text = egui::RichText::new(label);
                if b.is_head {
                    text = text.strong();
                }
                let resp = ui.selectable_label(selected, text);
                let resp = if b.ahead > 0 || b.behind > 0 {
                    resp.on_hover_text(format!("{} ahead, {} behind {}", b.ahead, b.behind, b.upstream.as_deref().unwrap_or("upstream")))
                } else {
                    resp
                };
                if resp.clicked() {
                    clicked = Some((b.name.clone(), b.oid));
                }
            }
        });
        let remote_count = snapshot.branches.iter().filter(|b| b.is_remote).count();
        egui::CollapsingHeader::new(format!("Remote ({remote_count})")).default_open(remote_count <= 30).show(ui, |ui| {
            for r in &snapshot.remotes {
                ui.weak(format!("  {r}"));
            }
            for b in snapshot.branches.iter().filter(|b| b.is_remote) {
                let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
                if ui.selectable_label(selected, format!("  {}", b.name)).clicked() {
                    clicked = Some((b.name.clone(), b.oid));
                }
            }
            if snapshot.branches.iter().all(|b| !b.is_remote) {
                ui.weak("  none");
            }
        });
        egui::CollapsingHeader::new("Tags").default_open(snapshot.tags.len() <= 20).show(ui, |ui| {
            for t in &snapshot.tags {
                let selected = app.sidebar_selected.as_deref() == Some(t.name.as_str());
                if ui.selectable_label(selected, format!("  {}", t.name)).clicked() {
                    clicked = Some((t.name.clone(), t.oid));
                }
            }
            if snapshot.tags.is_empty() {
                ui.weak("  none");
            }
        });
        egui::CollapsingHeader::new("Stashes").default_open(true).show(ui, |ui| {
            for s in &snapshot.stashes {
                let selected = app.sidebar_selected.as_deref() == Some(s.message.as_str());
                if ui.selectable_label(selected, format!("  {}: {}", s.index, s.message)).clicked() {
                    clicked = Some((s.message.clone(), s.oid));
                }
            }
            if snapshot.stashes.is_empty() {
                ui.weak("  none");
            }
        });

        if let Some((name, oid)) = clicked {
            app.sidebar_selected = Some(name);
            app.focus = Pane::Sidebar;
            if let Some(idx) = snapshot.commits.iter().position(|c| c.oid == oid) {
                app.select(Selection::Commit(idx));
                app.scroll_to_selection = true;
            } else {
                app.toast("commit not in the loaded log", false);
            }
        }
    });
}
