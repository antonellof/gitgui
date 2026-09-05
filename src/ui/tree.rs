//! Sidebar file tree: the whole working tree, not only changed files.
//! Directories are listed lazily by the git worker (`Command::ListDir`) so a
//! huge repository costs nothing until a folder is expanded. Changed files
//! carry their status color; ignored entries are dimmed.

use std::collections::HashMap;

use egui::Color32;

use crate::git::ops::Command;
use crate::git::repo::{DiffTarget, DirEntry, FileKind};
use crate::ui::app::{App, Pane, Selection};
use crate::ui::icons;
use crate::ui::theme::Theme;

/// What a row asked for; applied after the tree is drawn.
enum Act {
    Toggle(String),
    Request(String),
    /// Select the row (and open the file in the built-in editor when `open`).
    Select { path: String, open: bool },
    External(String),
    Preview(String),
    /// Select the file in the working tree lists to show its diff.
    Changes(String),
    Stage(String),
    Copy(String),
}

/// Per-file working tree state, for the row color and the menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Clean,
    Modified,
    Added,
    Deleted,
    Conflicted,
    Staged,
}

fn status_map(app: &App) -> HashMap<String, Status> {
    let mut m = HashMap::new();
    for f in &app.snapshot.staged {
        m.insert(f.path.clone(), Status::Staged);
    }
    for f in &app.snapshot.unstaged {
        let s = match f.kind {
            FileKind::Untracked | FileKind::Added => Status::Added,
            FileKind::Deleted => Status::Deleted,
            FileKind::Conflicted => Status::Conflicted,
            _ => Status::Modified,
        };
        m.insert(f.path.clone(), s);
    }
    for f in &app.snapshot.conflicted {
        m.insert(f.path.clone(), Status::Conflicted);
    }
    m
}

fn status_color(theme: &Theme, s: Status) -> Option<Color32> {
    match s {
        Status::Clean => None,
        Status::Modified => Some(theme.graph[2]),
        Status::Added => Some(theme.add_fg),
        Status::Deleted => Some(theme.del_fg),
        Status::Conflicted => Some(theme.error),
        Status::Staged => Some(theme.ok),
    }
}

/// Directories containing a changed file get a dot so the change is visible
/// while the folder is collapsed.
fn dir_has_changes(status: &HashMap<String, Status>, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    status.keys().any(|p| p.starts_with(&prefix))
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let theme = app.theme.clone();
    let status = status_map(app);
    let cmux = crate::split::is_cmux();
    let busy = app.busy > 0;
    let mut acts: Vec<Act> = Vec::new();
    egui::CollapsingHeader::new("Files")
        .default_open(true)
        .show(ui, |ui| {
            if !app.tree.contains_key("") {
                ui.weak("  loading");
                if !app.tree_requested.contains("") {
                    acts.push(Act::Request(String::new()));
                }
                return;
            }
            let ctx = Ctx {
                app,
                theme: &theme,
                status: &status,
                cmux,
                busy,
            };
            draw_dir(&ctx, ui, "", 0, &mut acts);
        });

    for act in acts {
        match act {
            Act::Toggle(d) => app.toggle_dir(&d),
            Act::Request(d) => app.request_dir(&d),
            Act::Select { path, open } => {
                app.tree_selected = Some(path.clone());
                app.focus = Pane::Sidebar;
                if open {
                    app.open_editor(path);
                }
            }
            Act::External(path) => {
                app.tree_selected = Some(path);
                app.focus = Pane::Sidebar;
                app.edit_selected_external();
            }
            Act::Preview(path) => {
                app.tree_selected = Some(path);
                app.focus = Pane::Sidebar;
                app.preview_selected_in_cmux();
            }
            Act::Changes(path) => {
                app.tree_selected = Some(path.clone());
                if app.editor.as_ref().is_some_and(|e| !e.dirty()) {
                    app.editor = None;
                }
                let target = if app.snapshot.unstaged.iter().any(|f| f.path == path) {
                    DiffTarget::WorkdirUnstaged(path)
                } else {
                    DiffTarget::Staged(path)
                };
                app.select(Selection::WorkingTree);
                app.select_file(Some(target));
                app.focus = Pane::Detail;
            }
            Act::Stage(path) => app.run(Command::Stage(vec![path])),
            Act::Copy(path) => {
                ui.ctx().copy_text(path);
                app.toast("copied path", false);
            }
        }
    }
}

struct Ctx<'a> {
    app: &'a App,
    theme: &'a Theme,
    status: &'a HashMap<String, Status>,
    cmux: bool,
    busy: bool,
}

const INDENT: f32 = 12.0;

fn draw_dir(ctx: &Ctx, ui: &mut egui::Ui, dir: &str, depth: usize, acts: &mut Vec<Act>) {
    let Some(entries) = ctx.app.tree.get(dir) else {
        ui.horizontal(|ui| {
            ui.add_space(INDENT * depth as f32 + 8.0);
            ui.weak("loading");
        });
        if !ctx.app.tree_requested.contains(dir) {
            acts.push(Act::Request(dir.to_owned()));
        }
        return;
    };
    if entries.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(INDENT * depth as f32 + 8.0);
            ui.weak("empty");
        });
    }
    for e in entries {
        if e.is_dir {
            draw_dir_row(ctx, ui, e, depth, acts);
            if ctx.app.tree_open.contains(&e.path) {
                draw_dir(ctx, ui, &e.path, depth + 1, acts);
            }
        } else {
            draw_file_row(ctx, ui, e, depth, acts);
        }
    }
}

fn draw_dir_row(ctx: &Ctx, ui: &mut egui::Ui, e: &DirEntry, depth: usize, acts: &mut Vec<Act>) {
    let open = ctx.app.tree_open.contains(&e.path);
    let selected = ctx.app.tree_selected.as_deref() == Some(e.path.as_str());
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().text_color();
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(INDENT * depth as f32);
            let tri = icons::disclosure(ui, open, if e.ignored { weak } else { strong });
            let mut text = egui::RichText::new(&e.name);
            if e.ignored {
                text = text.color(weak);
            }
            let label = ui.selectable_label(selected, text);
            if dir_has_changes(ctx.status, &e.path) {
                let c = ctx.theme.graph[2];
                ui.label(egui::RichText::new("•").color(c).small());
            }
            if tri.clicked() {
                acts.push(Act::Toggle(e.path.clone()));
            }
            label
        })
        .inner;
    if resp.clicked() {
        acts.push(Act::Toggle(e.path.clone()));
        acts.push(Act::Select {
            path: e.path.clone(),
            open: false,
        });
    }
    resp.context_menu(|ui| {
        if ui.button(if open { "Collapse" } else { "Expand" }).clicked() {
            acts.push(Act::Toggle(e.path.clone()));
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            acts.push(Act::Request(e.path.clone()));
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            acts.push(Act::Copy(e.path.clone()));
            ui.close();
        }
    });
}

fn draw_file_row(ctx: &Ctx, ui: &mut egui::Ui, e: &DirEntry, depth: usize, acts: &mut Vec<Act>) {
    let selected = ctx.app.tree_selected.as_deref() == Some(e.path.as_str());
    let status = ctx.status.get(&e.path).copied().unwrap_or(Status::Clean);
    let weak = ui.visuals().weak_text_color();
    let mut text = egui::RichText::new(&e.name);
    if let Some(c) = status_color(ctx.theme, status) {
        text = text.color(c);
    } else if e.ignored {
        text = text.color(weak);
    }
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // Align with directory names: the disclosure triangle's width.
            ui.add_space(INDENT * depth as f32 + ui.spacing().interact_size.y * 0.5 + 4.0);
            ui.selectable_label(selected, text)
        })
        .inner;
    let resp = match status {
        Status::Clean => resp.on_hover_text(&e.path),
        Status::Modified => resp.on_hover_text(format!("{} (modified)", e.path)),
        Status::Added => resp.on_hover_text(format!("{} (new)", e.path)),
        Status::Deleted => resp.on_hover_text(format!("{} (deleted)", e.path)),
        Status::Conflicted => resp.on_hover_text(format!("{} (conflict)", e.path)),
        Status::Staged => resp.on_hover_text(format!("{} (staged)", e.path)),
    };
    if resp.clicked() {
        acts.push(Act::Select {
            path: e.path.clone(),
            open: true,
        });
    }
    let path = e.path.clone();
    let busy = ctx.busy;
    let cmux = ctx.cmux;
    resp.context_menu(|ui| {
        let item = |ui: &mut egui::Ui, enabled: bool, label: &str, tip: &str| -> bool {
            let r = ui.add_enabled(enabled, egui::Button::new(label));
            let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
            let clicked = r.clicked();
            if clicked {
                ui.close();
            }
            clicked
        };
        if item(ui, true, "Edit", "e, built-in editor with syntax colors") {
            acts.push(Act::Select {
                path: path.clone(),
                open: true,
            });
        }
        if item(ui, true, "Open in $EDITOR", "Shift+E, new terminal split; set with --editor or git config gitgui.editor") {
            acts.push(Act::External(path.clone()));
        }
        if cmux && item(ui, true, "Preview in cmux", "Shift+O, cmux file preview tab") {
            acts.push(Act::Preview(path.clone()));
        }
        if status != Status::Clean {
            ui.separator();
            if item(ui, true, "Show changes", "select the file in the working tree lists") {
                acts.push(Act::Changes(path.clone()));
            }
            if matches!(status, Status::Modified | Status::Added | Status::Deleted)
                && item(ui, !busy, "Stage", "s")
            {
                acts.push(Act::Stage(path.clone()));
            }
        }
        ui.separator();
        if item(ui, true, "Copy path", "") {
            acts.push(Act::Copy(path.clone()));
        }
    });
}
