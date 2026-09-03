//! Branch picker modal content (VS Code style).

use egui::{FontId, RichText, Ui, Vec2};

use crate::git::repo::RepoSnapshot;
use crate::ui::theme::Theme;

pub enum BranchPickerAction {
    Select(String),
    CreateNew,
    PublishGithub,
}

pub fn has_origin(snapshot: &RepoSnapshot) -> bool {
    snapshot.remotes.iter().any(|r| r == "origin")
}

pub fn default_github_repo_name(snapshot: &RepoSnapshot) -> String {
    snapshot
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository")
        .to_owned()
}

pub fn branch_matches(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}

pub fn show(
    ui: &mut Ui,
    snapshot: &RepoSnapshot,
    theme: &Theme,
    filter: &mut String,
    busy: bool,
) -> Option<BranchPickerAction> {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 40.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(filter)
                    .hint_text("Search branches by name")
                    .font(FontId::proportional(15.0))
                    .margin(egui::vec2(10.0, 8.0))
                    .desired_width(f32::INFINITY),
            );
            if !resp.has_focus() && !ui.ctx().egui_wants_keyboard_input() {
                resp.request_focus();
            }
        },
    );
    ui.add_space(6.0);

    let current = snapshot
        .head
        .as_ref()
        .and_then(|h| h.branch_name.clone());

    let mut action = None;
    egui::ScrollArea::vertical()
        .id_salt("branch_picker_scroll")
        .max_height(320.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(440.0);
            let locals: Vec<_> = snapshot
                .branches
                .iter()
                .filter(|b| !b.is_remote && branch_matches(&b.name, filter))
                .collect();
            let remotes: Vec<_> = snapshot
                .branches
                .iter()
                .filter(|b| b.is_remote && branch_matches(&b.name, filter))
                .collect();

            if locals.is_empty() && remotes.is_empty() {
                ui.weak("No matching branches");
            }

            if !locals.is_empty() {
                ui.label(RichText::new("Local").weak());
                for b in locals {
                    if branch_row(ui, theme, &b.name, b.is_head, b.ahead, b.behind, busy) {
                        action = Some(BranchPickerAction::Select(b.name.clone()));
                    }
                }
                ui.add_space(4.0);
            }

            if !remotes.is_empty() {
                ui.label(RichText::new("Remote").weak());
                for b in remotes {
                    let selected = current.as_deref() == Some(b.name.as_str());
                    if branch_row(ui, theme, &b.name, selected, b.ahead, b.behind, busy) {
                        action = Some(BranchPickerAction::Select(b.name.clone()));
                    }
                }
            }
        });

    ui.add_space(6.0);
    ui.separator();
    if ui
        .add_enabled(!busy, egui::Button::new("+ Create new branch"))
        .clicked()
    {
        return Some(BranchPickerAction::CreateNew);
    }
    if !has_origin(snapshot)
        && ui
            .add_enabled(
                !busy,
                egui::Button::new("Publish to GitHub (create repo and push)"),
            )
            .on_hover_text("Uses the GitHub CLI (gh). Requires gh auth login.")
            .clicked()
    {
        return Some(BranchPickerAction::PublishGithub);
    }

    action
}

fn branch_row(
    ui: &mut Ui,
    theme: &Theme,
    name: &str,
    is_current: bool,
    ahead: usize,
    behind: usize,
    busy: bool,
) -> bool {
    let label = if is_current {
        format!("* {name}")
    } else {
        format!("  {name}")
    };
    let mut text = RichText::new(label).monospace();
    if is_current {
        text = text.strong().color(theme.branch_pill);
    }
    let mut resp = ui.add_enabled(!busy, egui::Button::new(text).frame(false));
    let tip = if ahead > 0 || behind > 0 {
        format!("{ahead} ahead, {behind} behind upstream")
    } else if is_current {
        "current branch".into()
    } else {
        String::new()
    };
    if !tip.is_empty() {
        resp = resp.on_hover_text(tip);
    }
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_filter_is_case_insensitive() {
        assert!(branch_matches("Feature/foo", "feat"));
        assert!(!branch_matches("main", "dev"));
        assert!(branch_matches("main", ""));
    }

    #[test]
    fn origin_detection() {
        use crate::git::repo::RepoSnapshot;
        let mut s = RepoSnapshot::default();
        assert!(!has_origin(&s));
        s.remotes.push("origin".into());
        assert!(has_origin(&s));
    }
}
