//! Sidebar file tree: the whole working tree, listed lazily by the git
//! worker (`Command::ListDir`). Changed files carry their status color,
//! ignored entries are dimmed, a collapsed folder with changes shows a dot.

use std::collections::HashMap;

use iced_core::{Alignment, Color, Length};
use iced_widget::{column, mouse_area, row, text, Space};

use crate::git::repo::{DirEntry, FileKind};
use crate::ui::app::{App, Element, Message, MenuKind, Pane};
use crate::ui::theme::Theme;
use crate::ui::widgets::row_button;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Clean,
    Modified,
    Added,
    Deleted,
    Conflicted,
    Staged,
}

pub fn status_map(app: &App) -> HashMap<String, Status> {
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

fn status_color(theme: &Theme, s: Status) -> Option<Color> {
    match s {
        Status::Clean => None,
        Status::Modified => Some(theme.graph[2]),
        Status::Added => Some(theme.add_fg),
        Status::Deleted => Some(theme.del_fg),
        Status::Conflicted => Some(theme.error),
        Status::Staged => Some(theme.ok),
    }
}

fn dir_has_changes(status: &HashMap<String, Status>, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    status.keys().any(|p| p.starts_with(&prefix))
}

const INDENT: f32 = 12.0;

/// The Files pane: a header with the refresh button, then the tree.
pub fn pane(app: &App) -> Element<'_> {
    let t = &app.theme;
    let header = row![
        text(app.snapshot.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned())
            .size(12)
            .color(t.weak)
            .wrapping(iced_core::text::Wrapping::None),
        Space::new().width(Length::Fill),
        crate::ui::widgets::small_button("refresh", Some(Message::TreeRequest(String::new()))),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .padding([4, 6]);
    column![
        header,
        iced_widget::scrollable(view(app)).spacing(6).width(Length::Fill).height(Length::Fill)
    ]
    .into()
}

pub fn view(app: &App) -> Element<'_> {
    let status = status_map(app);
    let mut col = column![].spacing(1).width(Length::Fill);
    if !app.tree.contains_key("") {
        col = col.push(row![Space::new().width(10), text("loading").size(12).color(app.theme.weak)].padding([2, 6]));
        return col.into();
    }
    push_dir(app, &status, "", 0, &mut col);
    col.into()
}

fn push_dir<'a>(
    app: &'a App,
    status: &HashMap<String, Status>,
    dir: &str,
    depth: usize,
    col: &mut iced_widget::Column<'a, Message, iced_core::Theme, crate::ui::app::Renderer>,
) {
    let t = &app.theme;
    let Some(entries) = app.tree.get(dir) else {
        col.push_ref_placeholder(depth, "loading", t);
        return;
    };
    if entries.is_empty() {
        col.push_ref_placeholder(depth, "empty", t);
    }
    let focused = app.focus == Pane::Files;
    for e in entries {
        if e.is_dir {
            col.push_ref(dir_row(app, status, e, depth, focused));
            if app.tree_open.contains(&e.path) {
                push_dir(app, status, &e.path, depth + 1, col);
            }
        } else {
            col.push_ref(file_row(app, status, e, depth, focused));
        }
    }
}

trait ColumnExt<'a> {
    fn push_ref(&mut self, e: Element<'a>);
    fn push_ref_placeholder(&mut self, depth: usize, label: &'a str, theme: &Theme);
}

impl<'a> ColumnExt<'a> for iced_widget::Column<'a, Message, iced_core::Theme, crate::ui::app::Renderer> {
    fn push_ref(&mut self, e: Element<'a>) {
        let this = std::mem::replace(self, column![]);
        *self = this.push(e);
    }

    fn push_ref_placeholder(&mut self, depth: usize, label: &'a str, theme: &Theme) {
        let e: Element<'a> = row![
            Space::new().width(INDENT * depth as f32 + 18.0),
            text(label).size(12).color(theme.weak)
        ]
        .padding([2, 6])
        .into();
        self.push_ref(e);
    }
}

fn dir_row<'a>(app: &'a App, status: &HashMap<String, Status>, e: &'a DirEntry, depth: usize, focused: bool) -> Element<'a> {
    let t = &app.theme;
    let open = app.tree_open.contains(&e.path);
    let selected = app.tree_selected.as_deref() == Some(e.path.as_str());
    let tri = if open { "▾" } else { "▸" };
    let color = if e.ignored { t.weak } else { t.text };
    let mut label = row![
        Space::new().width(INDENT * depth as f32),
        text(tri).size(12).color(color),
        text(&e.name).size(13).color(color).wrapping(iced_core::text::Wrapping::None),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if dir_has_changes(status, &e.path) {
        label = label.push(text("•").size(12).color(t.graph[2]));
    }
    let btn = row_button(label, selected, focused, Message::TreeToggle(e.path.clone()));
    mouse_area(btn)
        .on_right_press(Message::MenuOpen(MenuKind::TreeDir(e.path.clone())))
        .into()
}

fn file_row<'a>(app: &'a App, status: &HashMap<String, Status>, e: &'a DirEntry, depth: usize, focused: bool) -> Element<'a> {
    let t = &app.theme;
    let selected = app.tree_selected.as_deref() == Some(e.path.as_str());
    let st = status.get(&e.path).copied().unwrap_or(Status::Clean);
    let color = status_color(t, st).unwrap_or(if e.ignored { t.weak } else { t.text });
    let label = row![
        Space::new().width(INDENT * depth as f32 + 14.0),
        text(&e.name).size(13).color(color).wrapping(iced_core::text::Wrapping::None),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let btn = row_button(label, selected, focused, Message::TreeOpen(e.path.clone()));
    mouse_area(btn)
        .on_right_press(Message::MenuOpen(MenuKind::TreeFile(e.path.clone())))
        .into()
}
