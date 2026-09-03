//! Branches, remotes, tags, stashes.

use crate::git::ops::Command;
use crate::git::repo::short_id;
use crate::ui::app::{App, Modal, Pane, Selection};

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
        let mut cmd: Option<Command> = None;
        let mut modal: Option<Modal> = None;
        let busy = app.busy > 0;

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
                if resp.double_clicked() && !b.is_head && !busy {
                    cmd = Some(Command::Checkout(b.name.clone()));
                }
                resp.context_menu(|ui| {
                    if ui.add_enabled(!b.is_head && !busy, egui::Button::new("Checkout")).clicked() {
                        cmd = Some(Command::Checkout(b.name.clone()));
                        ui.close();
                    }
                    if ui.button("New branch from here").clicked() {
                        modal = Some(Modal::NewBranch { name: String::new(), from: b.oid, from_label: b.name.clone(), checkout: true });
                        ui.close();
                    }
                    if ui.add_enabled(!b.is_head && !busy, egui::Button::new("Delete")).clicked() {
                        modal = Some(Modal::DeleteBranch(b.name.clone()));
                        ui.close();
                    }
                    if ui.button("Copy name").clicked() {
                        ui.ctx().copy_text(b.name.clone());
                        ui.close();
                    }
                });
            }
        });
        let remote_count = snapshot.branches.iter().filter(|b| b.is_remote).count();
        egui::CollapsingHeader::new(format!("Remote ({remote_count})")).default_open(remote_count <= 30).show(ui, |ui| {
            for r in &snapshot.remotes {
                ui.weak(format!("  {r}"));
            }
            for b in snapshot.branches.iter().filter(|b| b.is_remote) {
                let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
                let resp = ui.selectable_label(selected, format!("  {}", b.name));
                if resp.clicked() {
                    clicked = Some((b.name.clone(), b.oid));
                }
                if resp.double_clicked() && !busy {
                    cmd = Some(Command::Checkout(b.name.clone()));
                }
                resp.context_menu(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Checkout (track)")).clicked() {
                        cmd = Some(Command::Checkout(b.name.clone()));
                        ui.close();
                    }
                    if ui.button("New branch from here").clicked() {
                        modal = Some(Modal::NewBranch { name: String::new(), from: b.oid, from_label: b.name.clone(), checkout: true });
                        ui.close();
                    }
                    if ui.button("Copy name").clicked() {
                        ui.ctx().copy_text(b.name.clone());
                        ui.close();
                    }
                });
            }
            if snapshot.branches.iter().all(|b| !b.is_remote) {
                ui.weak("  none");
            }
        });
        egui::CollapsingHeader::new("Tags").default_open(snapshot.tags.len() <= 20).show(ui, |ui| {
            for t in &snapshot.tags {
                let selected = app.sidebar_selected.as_deref() == Some(t.name.as_str());
                let resp = ui.selectable_label(selected, format!("  {}", t.name));
                if resp.clicked() {
                    clicked = Some((t.name.clone(), t.oid));
                }
                resp.context_menu(|ui| {
                    if ui.button("New branch from here").clicked() {
                        modal = Some(Modal::NewBranch { name: String::new(), from: t.oid, from_label: t.name.clone(), checkout: true });
                        ui.close();
                    }
                    if ui.button("Copy name").clicked() {
                        ui.ctx().copy_text(t.name.clone());
                        ui.close();
                    }
                });
            }
            if snapshot.tags.is_empty() {
                ui.weak("  none");
            }
        });
        egui::CollapsingHeader::new("Stashes").default_open(true).show(ui, |ui| {
            for s in &snapshot.stashes {
                let selected = app.sidebar_selected.as_deref() == Some(s.message.as_str());
                let resp = ui.selectable_label(selected, format!("  {}: {}", s.index, s.message));
                if resp.clicked() {
                    clicked = Some((s.message.clone(), s.oid));
                }
                resp.context_menu(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Pop")).clicked() {
                        cmd = Some(Command::StashPop(s.index));
                        ui.close();
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Drop")).clicked() {
                        modal = Some(Modal::DropStash(s.index));
                        ui.close();
                    }
                });
            }
            if ui.add_enabled(!busy && snapshot.is_dirty(), egui::Button::new("Stash changes").small()).clicked() {
                modal = Some(Modal::StashPush { message: String::new() });
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
                app.toast(format!("{} is not in the loaded log", short_id(oid)), false);
            }
        }
        if let Some(c) = cmd {
            app.run(c);
        }
        if modal.is_some() {
            app.modal = modal;
        }
    });
}
