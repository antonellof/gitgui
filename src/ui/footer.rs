//! Footer: branch switcher, counts, merge / rebase banner, last operation,
//! fetch / pull / push / refresh / quit. Plus the network log panel.

use iced_core::{Alignment, Background, Border, Font, Length};
use iced_widget::{column, container, mouse_area, row, scrollable, text, Space};

use crate::git::ops::{Command, StateAction};
use crate::git::repo::RepoState;
use crate::ui::app::{App, Element, Message};
use crate::ui::widgets::{self, small_button};

pub fn view(app: &App) -> Element<'_> {
    let t = &app.theme;
    let s = &app.snapshot;
    let busy = app.busy > 0;
    let compact = app.window.width < 1000.0;
    let label = |full: &'static str, short: &'static str| if compact { short } else { full };
    let mut r = row![].spacing(8).align_y(Alignment::Center).padding([4, 8]);

    r = r.push(
        row![
            text("gitgui").size(12).color(t.strong),
            text(concat!("v", env!("CARGO_PKG_VERSION"))).size(11).color(t.weak),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    );
    r = r.push(text("|").size(12).color(t.border));

    if !app.no_repo {
        let name = match &s.head {
            Some(h) => h.branch_name.clone().unwrap_or_else(|| {
                h.oid
                    .map(|o| format!("detached {}", crate::git::repo::short_id(o)))
                    .unwrap_or_else(|| "no HEAD".into())
            }),
            None => "no HEAD".into(),
        };
        r = r.push(small_button(format!("{name}  ▾"), (!busy && app.modal.is_none()).then_some(Message::OpenBranchPicker)));
        if let Some(b) = s.branches.iter().find(|b| b.is_head) {
            if b.ahead > 0 || b.behind > 0 {
                let ab = match (b.ahead, b.behind) {
                    (a, 0) => format!("{a} ahead"),
                    (0, bh) => format!("{bh} behind"),
                    (a, bh) => format!("{a} ahead, {bh} behind"),
                };
                r = r.push(text(ab).size(12).color(t.weak));
            }
        }
        r = r.push(text(format!("{} unstaged, {} staged", s.unstaged.len(), s.staged.len())).size(12).color(t.weak));
        if s.state != RepoState::Clean {
            let mut label = s.state.label().to_owned();
            if let Some((done, total)) = s.rebase_progress {
                label.push_str(&format!(" {done}/{total}"));
            }
            if !compact {
                label.push_str(" in progress");
            }
            let banner = row![
                text(label).size(12).color(t.strong),
                small_button("Continue", (!busy).then_some(Message::StateAction(StateAction::Continue))),
                small_button("Abort", (!busy).then_some(Message::StateAction(StateAction::Abort))),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            let bg = crate::ui::theme::alpha(t.error, 0.35);
            r = r.push(container(banner).padding([2, 8]).style(move |_| container::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: 5.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }));
        }
        if let Some(op) = &app.last_op {
            let short: String = op.chars().take(if compact { 24 } else { 60 }).collect();
            r = r.push(container(text(short).size(12).color(t.weak).wrapping(iced_core::text::Wrapping::None)).clip(true));
        }
    } else {
        r = r.push(text(app.repo_path.display().to_string()).size(12).font(Font::MONOSPACE).color(t.weak));
    }
    for kind in app.hidden_panes() {
        r = r.push(small_button(format!("+ {}", kind.title()), Some(Message::PaneShow(kind))));
    }
    r = r.push(Space::new().width(Length::Fill));
    if app.show_debug {
        r = r.push(text(format!("{:.1} ms {} x{}", app.frame_ms, app.transport, app.scale)).size(11).color(t.weak));
    }
    if !app.no_repo {
        let fetch = small_button(label("Fetch  f", "Fetch"), (!busy).then_some(Message::Run(Command::Fetch)));
        let pull = mouse_area(small_button(label("Pull  p", "Pull"), (!busy).then_some(Message::Run(Command::Pull))))
            .on_right_press(Message::Run(Command::PullRebase));
        let push = mouse_area(small_button(label("Push  P", "Push"), (!busy).then_some(Message::Run(Command::Push)))).on_right_press(
            Message::Confirm(
                "Force push",
                "Push with --force-with-lease? Remote commits not in your branch are overwritten.".into(),
                "Force push",
                Command::ForcePush,
            ),
        );
        r = r.push(fetch).push(pull).push(push);
        r = r.push(small_button(label("Refresh  r", "Refresh"), Some(Message::Refresh)));
    }
    r = r.push(small_button(label("Change folder", "Folder"), (app.modal.is_none()).then_some(Message::OpenFolderDialog)));
    r = r.push(small_button(label("Help  ?", "?"), Some(Message::OpenHelp)));
    r = r.push(small_button(label("Quit  q", "Quit"), Some(Message::Quit)));
    let bg = t.panel;
    container(r)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            ..Default::default()
        })
        .into()
}

pub fn net_log(app: &App) -> Element<'_> {
    let t = &app.theme;
    let title = format!("git {}{}", app.net.label, if app.net.running { " (running)" } else { "" });
    let mut lines = column![].spacing(0);
    for l in app.net.lines.iter().rev().take(200).collect::<Vec<_>>().into_iter().rev() {
        lines = lines.push(text(l).size(12).font(Font::MONOSPACE));
    }
    let header = row![
        text(title).size(12).color(t.strong),
        Space::new().width(Length::Fill),
        small_button("close", Some(Message::NetClose)),
    ]
    .align_y(Alignment::Center)
    .spacing(6);
    let bg = t.well;
    container(column![header, scrollable(lines).height(Length::Fixed(110.0)).anchor_bottom()].spacing(4))
        .padding(6)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            ..Default::default()
        })
        .into()
}

#[allow(dead_code)]
fn unused() -> Element<'static> {
    widgets::button("", None)
}
