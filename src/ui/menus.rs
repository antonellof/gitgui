//! Right-click menus. Each builder only reads the app and returns what was
//! picked; the caller applies it once the borrow of the list is over.

use crate::git::rebase::TodoAction;
use crate::ui::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMenu {
    NewBranch,
    Tag,
    CherryPick,
    Revert,
    Reset,
    CheckoutDetached,
    Rewrite(TodoAction),
    Reword,
    Autosquash,
    CreateFixup,
    CopyHash,
    CopyMessage,
    OpenBrowser,
}

pub fn commit_menu(ui: &mut egui::Ui, app: &App, idx: usize) -> Option<CommitMenu> {
    let busy = app.busy > 0;
    let mut picked = None;
    let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, tip: &str, action: CommitMenu| {
        let resp = ui.add_enabled(enabled && !busy, egui::Button::new(label));
        let resp = if tip.is_empty() {
            resp
        } else {
            resp.on_hover_text(tip)
        };
        if resp.clicked() {
            picked = Some(action);
            ui.close();
        }
    };
    let head = app
        .snapshot
        .head
        .as_ref()
        .and_then(|h| h.oid)
        .is_some_and(|h| app.snapshot.commits.get(idx).is_some_and(|c| c.oid == h));
    let info = app.rewrite_info(idx);
    let rewrite = info.is_some();
    let has_older = info.is_some_and(|i| i.has_older);
    let has_newer = info.is_some_and(|i| !i.is_head);
    item(ui, true, "New branch here", "n", CommitMenu::NewBranch);
    item(ui, true, "Tag here", "Shift+T", CommitMenu::Tag);
    item(ui, true, "Check out (detached HEAD)", "", CommitMenu::CheckoutDetached);
    ui.separator();
    item(ui, !head, "Cherry-pick onto HEAD", "Shift+C", CommitMenu::CherryPick);
    item(ui, true, "Revert", "t", CommitMenu::Revert);
    item(ui, true, "Reset current branch here", "g, soft / mixed / hard", CommitMenu::Reset);
    ui.separator();
    item(ui, rewrite, "Reword", "Shift+R", CommitMenu::Reword);
    item(ui, rewrite && has_older, "Squash into commit below", "keeps both messages", CommitMenu::Rewrite(TodoAction::Squash));
    item(ui, rewrite && has_older, "Fixup into commit below", "discards this message", CommitMenu::Rewrite(TodoAction::Fixup));
    item(ui, rewrite && !head, "Drop", "d", CommitMenu::Rewrite(TodoAction::Drop));
    item(ui, rewrite && has_newer, "Move up", "Shift+K", CommitMenu::Rewrite(TodoAction::MoveUp));
    item(ui, rewrite && has_older, "Move down", "Shift+J", CommitMenu::Rewrite(TodoAction::MoveDown));
    item(ui, rewrite && !head, "Edit (stop the rebase here)", "continue with m when done", CommitMenu::Rewrite(TodoAction::Edit));
    ui.separator();
    item(ui, !app.snapshot.staged.is_empty(), "Create fixup commit for this", "commits the staged changes as fixup!", CommitMenu::CreateFixup);
    item(ui, rewrite, "Apply fixup commits above", "rebase --autosquash", CommitMenu::Autosquash);
    ui.separator();
    item(ui, true, "Copy hash", "y", CommitMenu::CopyHash);
    item(ui, true, "Copy message", "", CommitMenu::CopyMessage);
    item(ui, app.web_remote().is_some(), "Open in browser", "o", CommitMenu::OpenBrowser);
    picked
}

pub fn apply_commit_menu(app: &mut App, ctx: &egui::Context, idx: usize, action: CommitMenu) {
    match action {
        CommitMenu::NewBranch => app.commit_new_branch(idx),
        CommitMenu::Tag => app.commit_tag(idx),
        CommitMenu::CherryPick => app.commit_cherry_pick(idx),
        CommitMenu::Revert => app.commit_revert(idx),
        CommitMenu::Reset => app.commit_reset(idx),
        CommitMenu::CheckoutDetached => app.commit_checkout_detached(idx),
        CommitMenu::Rewrite(a) => app.commit_rewrite(idx, a),
        CommitMenu::Reword => app.commit_reword(idx),
        CommitMenu::Autosquash => app.commit_autosquash(idx),
        CommitMenu::CreateFixup => app.commit_create_fixup(idx),
        CommitMenu::CopyHash => app.commit_copy_hash(ctx, idx),
        CommitMenu::CopyMessage => app.commit_copy_message(ctx, idx),
        CommitMenu::OpenBrowser => app.commit_open_in_browser(idx),
    }
}
