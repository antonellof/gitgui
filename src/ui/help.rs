//! Keyboard reference shown by the `?` dialog. One table, so the dialog and
//! the docs cannot drift apart.

/// (section, key, action)
pub const KEYS: &[(&str, &str, &str)] = &[
    ("Navigation", "j / k, Down / Up", "move selection"),
    ("Navigation", "PageDown / PageUp, Home / End", "jump in the list"),
    ("Navigation", "Tab", "cycle focus: sidebar, log, detail"),
    ("Navigation", "Enter", "open selection / check out branch"),
    ("Navigation", "/", "filter commits (summary, author, hash)"),
    ("Navigation", "Escape", "clear filter, search, selection; close dialog"),
    ("Navigation", "?", "this help"),
    ("Navigation", "q, Ctrl+C", "quit"),
    ("Working tree", "s / u", "stage / unstage the selected file or lines"),
    ("Working tree", "Space", "toggle the selected file staged"),
    ("Working tree", "a / Shift+A", "stage all / unstage all"),
    ("Working tree", "d", "discard the selected file or lines (asks)"),
    ("Working tree", "Shift+D", "discard every change (asks)"),
    ("Working tree", "i", "add the selected untracked file to .gitignore"),
    ("Working tree", "e", "edit the selected file (built-in editor)"),
    ("Working tree", "Shift+E", "open the selected file in your editor (--editor, gitgui.editor, $EDITOR)"),
    ("Working tree", "Shift+O", "open the selected file in a cmux preview tab"),
    ("Editor", "Ctrl+S", "save"),
    ("Editor", "Escape", "close (asks when unsaved)"),
    ("Editor", "Ctrl+Z / Ctrl+Y", "undo / redo"),
    ("Working tree", "c", "focus the commit message"),
    ("Working tree", "Ctrl+Enter", "commit"),
    ("Working tree", "Ctrl+Shift+Enter", "commit and push"),
    ("Working tree", "Shift+S", "stash"),
    ("Diff", "click, Shift+click, drag", "select lines"),
    ("Diff", "Ctrl+F", "search in the diff"),
    ("Diff", "n / Shift+N", "next / previous match"),
    ("Diff", "{ / }", "less / more context"),
    ("Diff", "Ctrl+W", "ignore whitespace"),
    ("Commits", "n", "new branch from the selected commit"),
    ("Commits", "Shift+T", "tag the selected commit"),
    ("Commits", "Shift+C", "cherry-pick the selected commit onto HEAD"),
    ("Commits", "t", "revert the selected commit"),
    ("Commits", "g", "reset the current branch to the selected commit"),
    ("Commits", "Shift+R", "reword the selected commit"),
    ("Commits", "d", "drop the selected commit"),
    ("Commits", "Shift+K / Shift+J", "move the selected commit up / down"),
    ("Commits", "y", "copy the commit hash"),
    ("Commits", "o", "open the commit in the browser"),
    ("Commits", "right click", "squash, fixup, edit, autosquash and more"),
    ("Remote", "f / p / Shift+P", "fetch / pull / push"),
    ("Remote", "r", "refresh"),
    ("Remote", "m", "continue, abort or skip a merge or rebase"),
];

pub fn show(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .id_salt("help_scroll")
        .max_height(420.0)
        .show(ui, |ui| {
            let mut section = "";
            egui::Grid::new("help_grid")
                .num_columns(2)
                .spacing([18.0, 3.0])
                .show(ui, |ui| {
                    for (sec, key, action) in KEYS {
                        if *sec != section {
                            section = sec;
                            ui.strong(*sec);
                            ui.end_row();
                        }
                        ui.monospace(*key);
                        ui.label(*action);
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.weak("Right click branches, remotes, tags, stashes, files and commits for more.");
        });
}
