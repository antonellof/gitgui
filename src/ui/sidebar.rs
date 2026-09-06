//! Branches, remotes, tags, stashes and the file tree. Right-click opens the
//! context menu for the row (`menu.rs`), double-click checks a branch out.

use iced_core::{Alignment, Length};
use iced_widget::{column, mouse_area, row, scrollable, text, Space};

use crate::git::ops::Command;
use crate::ui::app::{App, Element, Message, MenuKind, Modal, Pane};
use crate::ui::widgets::{self, row_button, section, small_button};
use crate::ui::tree;

pub fn view(app: &App) -> Element<'_> {
    let t = &app.theme;
    let s = &app.snapshot;
    let busy = app.busy > 0;
    let focused = app.focus == Pane::Sidebar;
    let open = |title: &str| !app.sidebar_collapsed.contains(title);
    let mut col = column![].spacing(1).width(Length::Fill);

    // Local branches.
    col = col.push(section(
        "Local",
        !open("Local"),
        Some(small_button("new", (!busy && s.head.is_some()).then_some(Message::OpenNewBranch))),
        t,
    ));
    for b in s.branches.iter().filter(|b| !b.is_remote && open("Local")) {
        let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
        let mut label = row![].spacing(6).align_y(Alignment::Center);
        if b.is_head {
            label = label.push(text("●").size(10).color(t.head_pill));
        } else {
            label = label.push(Space::new().width(10));
        }
        let mut name = text(&b.name).size(13).wrapping(iced_core::text::Wrapping::None);
        if b.is_head {
            name = name.color(t.strong);
        }
        label = label.push(name);
        if b.ahead > 0 || b.behind > 0 {
            label = label.push(Space::new().width(Length::Fill));
            let ab = match (b.ahead, b.behind) {
                (a, 0) => format!("{a} ahead"),
                (0, bh) => format!("{bh} behind"),
                (a, bh) => format!("{a} ahead, {bh} behind"),
            };
            label = label.push(text(ab).size(11).color(t.weak));
        }
        let btn = row_button(label, selected, focused, Message::SidebarSelect(b.name.clone(), b.oid));
        let name = b.name.clone();
        let mut area = mouse_area(btn).on_right_press(Message::MenuOpen(MenuKind::Branch(name.clone())));
        if !b.is_head && !busy {
            area = area.on_double_click(Message::Switch(name));
        }
        col = col.push(area);
    }

    // Remotes and remote branches.
    let remote_count = s.branches.iter().filter(|b| b.is_remote).count();
    let remote_title: &'static str = "Remote";
    col = col.push(section(
        remote_title,
        !open("Remote"),
        Some(small_button(
            "add",
            (!busy).then_some(Message::Input(
                crate::ui::app::InputKind::RemoteAdd,
                if s.remotes.is_empty() { "origin".into() } else { String::new() },
                String::new(),
            )),
        )),
        t,
    ));
    for r in s.remotes.iter().filter(|_| open("Remote")) {
        let url = s
            .remote_urls
            .iter()
            .find(|(n, _)| n == r)
            .map(|(_, u)| u.clone())
            .unwrap_or_default();
        let label = row![text(r).size(12).color(t.weak), text(url).size(11).color(t.line_no).wrapping(iced_core::text::Wrapping::None)]
            .spacing(8)
            .align_y(Alignment::Center);
        let btn = row_button(label, false, focused, Message::Nothing);
        col = col.push(mouse_area(btn).on_right_press(Message::MenuOpen(MenuKind::Remote(r.clone()))));
    }
    if !open("Remote") {
        // collapsed
    } else if remote_count > 0 {
        for b in s.branches.iter().filter(|b| b.is_remote) {
            let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
            let label = row![Space::new().width(10), text(&b.name).size(13)]
                .spacing(6)
                .align_y(Alignment::Center);
            let btn = row_button(label, selected, focused, Message::SidebarSelect(b.name.clone(), b.oid));
            let name = b.name.clone();
            let mut area = mouse_area(btn).on_right_press(Message::MenuOpen(MenuKind::RemoteBranch(name.clone())));
            if !busy {
                area = area.on_double_click(Message::Switch(name));
            }
            col = col.push(area);
        }
    } else if s.remotes.is_empty() {
        col = col.push(
            row![Space::new().width(10), widgets::weak("none", t)].padding([2, 6]),
        );
        if !app.has_origin() {
            col = col.push(
                row![
                    Space::new().width(10),
                    small_button("publish to GitHub", (!busy).then_some(Message::OpenPublish))
                ]
                .padding([2, 6]),
            );
        }
    }

    // Tags.
    let head_oid = s.head.as_ref().and_then(|h| h.oid);
    col = col.push(section(
        "Tags",
        !open("Tags"),
        head_oid.map(|oid| {
            small_button(
                "new",
                (!busy).then_some(Message::Input(
                    crate::ui::app::InputKind::Tag {
                        oid,
                        label: format!("HEAD ({})", crate::git::repo::short_id(oid)),
                    },
                    String::new(),
                    String::new(),
                )),
            )
        }),
        t,
    ));
    for tag in s.tags.iter().filter(|_| open("Tags")) {
        let selected = app.sidebar_selected.as_deref() == Some(tag.name.as_str());
        let label = row![Space::new().width(10), text(&tag.name).size(13)]
            .spacing(6)
            .align_y(Alignment::Center);
        let btn = row_button(label, selected, focused, Message::SidebarSelect(tag.name.clone(), tag.oid));
        col = col.push(mouse_area(btn).on_right_press(Message::MenuOpen(MenuKind::Tag(tag.name.clone()))));
    }
    if s.tags.is_empty() && open("Tags") {
        col = col.push(row![Space::new().width(10), widgets::weak("none", t)].padding([2, 6]));
    }

    // Stashes.
    col = col.push(section(
        "Stashes",
        !open("Stashes"),
        Some(small_button(
            "stash",
            (!busy && s.is_dirty()).then_some(Message::OpenStashDialog),
        )),
        t,
    ));
    for st in s.stashes.iter().filter(|_| open("Stashes")) {
        let selected = app.sidebar_selected.as_deref() == Some(st.message.as_str());
        let label = row![
            Space::new().width(10),
            text(format!("{}: {}", st.index, st.message)).size(13)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let btn = row_button(label, selected, focused, Message::SidebarSelect(st.message.clone(), st.oid));
        col = col.push(mouse_area(btn).on_right_press(Message::MenuOpen(MenuKind::Stash(st.index))));
    }
    if s.stashes.is_empty() && open("Stashes") {
        col = col.push(row![Space::new().width(10), widgets::weak("none", t)].padding([2, 6]));
    }

    // Files.
    col = col.push(section(
        "Files",
        !open("Files"),
        Some(small_button("refresh", Some(Message::TreeRequest(String::new())))),
        t,
    ));
    if open("Files") {
        col = col.push(tree::view(app));
    }

    scrollable(col.padding([0, 4]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Sidebar-specific messages that need the app: none for now, kept so the
/// tree module can reuse the menu path.
#[allow(dead_code)]
pub fn checkout(name: String) -> Message {
    Message::Run(Command::Checkout(name))
}

#[allow(dead_code)]
pub fn delete_branch(name: String) -> Message {
    Message::Modal(Modal::DeleteBranch(name))
}
