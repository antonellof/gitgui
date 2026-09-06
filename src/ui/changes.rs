//! Working tree: unstaged and staged lists with the commit box, or the file
//! list of the selected commit with its message body.

use iced_core::keyboard;
use iced_core::{Alignment, Font, Length};
use iced_widget::text_editor::Binding;
use iced_widget::{checkbox, column, container, mouse_area, row, scrollable, text, Space};

use crate::git::ops::Command;
use crate::git::repo::{DiffTarget, FileKind, FileStatus};
use crate::ui::app::{App, Element, Message, MenuKind, Pane, Selection};
use crate::ui::widgets::{self, primary_button, row_button, small_button};

pub fn view(app: &App) -> Element<'_> {
    match app.selection {
        Selection::WorkingTree => worktree(app),
        Selection::Commit(i) => commit(app, i),
    }
}

fn status_color(app: &App, kind: FileKind) -> iced_core::Color {
    let t = &app.theme;
    match kind {
        FileKind::Added | FileKind::Untracked => t.add_fg,
        FileKind::Deleted => t.del_fg,
        FileKind::Conflicted => t.error,
        FileKind::Renamed | FileKind::TypeChange => t.graph[7],
        FileKind::Modified => t.graph[2],
    }
}

fn file_row<'a>(app: &'a App, f: &'a FileStatus, target: DiffTarget, staged: bool) -> Element<'a> {
    let t = &app.theme;
    let selected = app.selected_file.as_ref() == Some(&target);
    let focused = app.focus == Pane::Changes;
    let busy = app.busy > 0;
    let path = f.path.clone();
    let action: Element<'a> = if f.kind == FileKind::Conflicted {
        text("!").size(12).color(t.error).into()
    } else if staged {
        small_button("-", (!busy).then_some(Message::Run(Command::Unstage(vec![path.clone()]))))
    } else {
        small_button("+", (!busy).then_some(Message::Run(Command::Stage(vec![path.clone()]))))
    };
    let label = row![
        action,
        text(f.kind.letter()).size(12).font(Font::MONOSPACE).color(status_color(app, f.kind)),
        text(&f.path).size(13).font(Font::MONOSPACE).wrapping(iced_core::text::Wrapping::None),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let btn = row_button(label, selected, focused, Message::SelectFile(target));
    mouse_area(btn)
        .on_right_press(Message::MenuOpen(MenuKind::File {
            path,
            staged,
            conflicted: f.kind == FileKind::Conflicted,
            untracked: f.kind == FileKind::Untracked,
        }))
        .into()
}

fn worktree(app: &App) -> Element<'_> {
    let t = &app.theme;
    let s = &app.snapshot;
    let busy = app.busy > 0;
    let mut col = column![].spacing(2).width(Length::Fill).height(Length::Fill);

    // Unstaged.
    let unstaged_n = s.unstaged.len() + s.conflicted.len();
    col = col.push(
        row![
            text(format!("Unstaged ({unstaged_n})")).size(13).color(t.strong),
            Space::new().width(Length::Fill),
            small_button("stage all", (!busy && !s.unstaged.is_empty()).then_some(Message::Run(Command::StageAll))),
            small_button("discard all", (!busy && s.is_dirty()).then_some(Message::DiscardAll)),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .padding([4, 6]),
    );
    let mut list = column![].spacing(1);
    for f in s.conflicted.iter().chain(s.unstaged.iter()) {
        list = list.push(file_row(app, f, DiffTarget::WorkdirUnstaged(f.path.clone()), false));
    }
    if unstaged_n == 0 {
        list = list.push(container(text("nothing to stage").size(12).color(t.weak)).padding([2, 12]));
    }
    col = col.push(scrollable(list.padding([0, 4])).height(Length::FillPortion(1)));

    // Staged.
    col = col.push(
        row![
            text(format!("Staged ({})", s.staged.len())).size(13).color(t.strong),
            Space::new().width(Length::Fill),
            small_button("unstage all", (!busy && !s.staged.is_empty()).then_some(Message::Run(Command::UnstageAll))),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .padding([4, 6]),
    );
    let mut list = column![].spacing(1);
    for f in &s.staged {
        list = list.push(file_row(app, f, DiffTarget::Staged(f.path.clone()), true));
    }
    if s.staged.is_empty() {
        list = list.push(container(text("nothing staged").size(12).color(t.weak)).padding([2, 12]));
    }
    col = col.push(scrollable(list.padding([0, 4])).height(Length::FillPortion(1)));

    // Commit box.
    let editor = iced_widget::TextEditor::new(&app.commit_msg)
        .id(widgets::COMMIT_BOX_ID.clone())
        .placeholder("Commit message")
        .on_action(Message::CommitMsg)
        .size(13)
        .padding(6)
        .height(Length::Fixed(56.0))
        .key_binding(|press| {
            if !matches!(press.status, iced_widget::text_editor::Status::Focused { .. }) {
                return None;
            }
            let mods = press.modifiers;
            match &press.key {
                keyboard::Key::Named(keyboard::key::Named::Enter) if mods.control() && mods.shift() => {
                    Some(Binding::Custom(Message::CommitAndPush))
                }
                keyboard::Key::Named(keyboard::key::Named::Enter) if mods.control() => {
                    Some(Binding::Custom(Message::Commit))
                }
                keyboard::Key::Named(keyboard::key::Named::Escape) => Some(Binding::Unfocus),
                _ => Binding::from_key_press(press),
            }
        })
        .style(widgets::text_editor_style);
    let can_commit = !busy && (!s.staged.is_empty() || app.amend);
    let author = if s.user_name.is_empty() {
        "no user.name configured".to_owned()
    } else {
        format!("{} <{}>", s.user_name, s.user_email)
    };
    let meta = row![
        checkbox(app.amend).label("amend").size(14).text_size(12).on_toggle(Message::ToggleAmend),
        container(text(author).size(11).color(t.weak).wrapping(iced_core::text::Wrapping::None))
            .width(Length::Fill)
            .clip(true),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let buttons = row![
        Space::new().width(Length::Fill),
        small_button("Commit & Push", can_commit.then_some(Message::CommitAndPush)),
        primary_button("Commit", can_commit.then_some(Message::Commit)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    col = col.push(column![editor, meta, buttons].spacing(6).padding(6));
    col.into()
}

fn commit(app: &App, idx: usize) -> Element<'_> {
    let t = &app.theme;
    let Some(c) = app.snapshot.commits.get(idx) else {
        return container(text("").size(12)).into();
    };
    let mut col = column![].spacing(4).width(Length::Fill).height(Length::Fill).padding(6);
    col = col.push(
        row![
            text(&c.short).size(12).font(Font::MONOSPACE).color(t.weak),
            text(&c.author).size(12).color(t.weak),
            Space::new().width(Length::Fill),
            small_button("copy hash", Some(Message::CommitAction(idx, crate::ui::app::CommitAction::CopyHash))),
            small_button("menu", Some(Message::MenuOpen(MenuKind::Commit(idx)))),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    col = col.push(text(&c.summary).size(14).color(t.strong));
    if !c.body.is_empty() {
        col = col.push(
            scrollable(container(text(&c.body).size(12).font(Font::MONOSPACE)).padding([4, 0]))
                .width(Length::Fill)
                .height(Length::Shrink)
                .spacing(2),
        );
    }
    let files = app.commit_files.get(&c.oid);
    let n = files.map(|f| f.len()).unwrap_or(0);
    col = col.push(text(format!("Files ({n})")).size(12).color(t.weak));
    let mut list = column![].spacing(1);
    match files {
        Some(files) => {
            for f in files {
                let target = DiffTarget::Commit(c.oid, f.path.clone());
                let selected = app.selected_file.as_ref() == Some(&target);
                let label = row![
                    text(f.kind.letter()).size(12).font(Font::MONOSPACE).color(status_color(app, f.kind)),
                    text(&f.path).size(13).font(Font::MONOSPACE),
                ]
                .spacing(6)
                .align_y(Alignment::Center);
                list = list.push(row_button(label, selected, app.focus == Pane::Changes, Message::SelectFile(target)));
            }
        }
        None => {
            list = list.push(text("loading").size(12).color(t.weak));
        }
    }
    col = col.push(scrollable(list).height(Length::Fill));
    col.into()
}
