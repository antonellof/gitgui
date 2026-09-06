//! Dialogs: a dimmed layer over the app with the dialog centered. Enter
//! confirms, Escape closes (handled in `App::key` while a modal is open).

use iced_core::{Alignment, Background, Color, Length};
use iced_widget::{checkbox, column, container, mouse_area, row, scrollable, text, text_input, Space};

use crate::git::actions::ResetKind;
use crate::git::ops::StateAction;
use crate::ui::app::{App, Element, InputKind, Message, Modal};
use crate::ui::widgets::{self, button, danger_button, primary_button, row_button};
use crate::ui::help;

pub fn branch_matches(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(&filter.to_lowercase())
}

fn input<'a>(placeholder: &'a str, value: &'a str, first: bool, on_input: fn(String) -> Message) -> Element<'a> {
    let mut i = text_input(placeholder, value)
        .on_input(on_input)
        .on_submit(Message::ModalConfirm)
        .size(13)
        .padding([5, 8])
        .style(widgets::text_input_style)
        .width(Length::Fill);
    if first {
        i = i.id(widgets::MODAL_INPUT_ID.clone());
    }
    i.into()
}

fn buttons<'a>(primary: Element<'a>, cancel_label: &'a str) -> Element<'a> {
    row![
        Space::new().width(Length::Fill),
        button(cancel_label, Some(Message::ModalClose)),
        primary,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

pub fn view<'a>(app: &'a App, modal: &'a Modal) -> Element<'a> {
    let t = &app.theme;
    let busy = app.busy > 0;
    let title: String = match modal {
        Modal::Discard(_) => "Discard changes".into(),
        Modal::NewBranch { .. } => "New branch".into(),
        Modal::DeleteBranch(_) => "Delete branch".into(),
        Modal::DropStash(_) => "Drop stash".into(),
        Modal::BranchPicker { .. } => "Switch branch".into(),
        Modal::CheckoutConfirm { .. } => "Uncommitted changes".into(),
        Modal::PublishGithub { .. } => "Publish to GitHub".into(),
        Modal::Confirm { title, .. } => (*title).into(),
        Modal::Input { kind, .. } => kind.title().into(),
        Modal::Reset { .. } => "Reset current branch".into(),
        Modal::StashOpts { .. } => "Stash changes".into(),
        Modal::StateMenu => "Operation in progress".into(),
        Modal::Help => "Keyboard shortcuts".into(),
        Modal::CloseEditor => "Unsaved changes".into(),
    };
    let body: Element<'a> = match modal {
        Modal::Discard(paths) => column![
            text(format!(
                "Throw away the changes to {}? This cannot be undone.",
                if paths.len() == 1 { paths[0].clone() } else { format!("{} files", paths.len()) }
            ))
            .size(13),
            buttons(danger_button("Discard", (!busy).then_some(Message::ModalConfirm)), "Cancel"),
        ]
        .spacing(12)
        .into(),
        Modal::NewBranch {
            name,
            from_label,
            checkout,
            ..
        } => column![
            text(format!("From {from_label}")).size(12).color(t.weak),
            input("branch name", name, true, Message::ModalValue),
            checkbox(*checkout).label("check out the new branch").size(14).text_size(12).on_toggle(Message::ModalCheckbox),
            buttons(
                primary_button("Create", (!busy && !name.trim().is_empty()).then_some(Message::ModalConfirm)),
                "Cancel"
            ),
        ]
        .spacing(10)
        .into(),
        Modal::DeleteBranch(name) => column![
            text(format!("Delete local branch {name}?")).size(13),
            buttons(danger_button("Delete", (!busy).then_some(Message::ModalConfirm)), "Cancel"),
        ]
        .spacing(12)
        .into(),
        Modal::DropStash(i) => column![
            text(format!("Drop stash@{{{i}}}? Its changes are lost.")).size(13),
            buttons(danger_button("Drop", (!busy).then_some(Message::ModalConfirm)), "Cancel"),
        ]
        .spacing(12)
        .into(),
        Modal::BranchPicker { filter } => {
            let mut list = column![].spacing(1);
            let current = app.snapshot.head.as_ref().and_then(|h| h.branch_name.clone());
            let mut shown = 0;
            for b in app.snapshot.branches.iter().filter(|b| branch_matches(&b.name, filter)) {
                let is_current = current.as_deref() == Some(b.name.as_str());
                let mut label = row![text(&b.name).size(13)].spacing(6).align_y(Alignment::Center);
                if is_current {
                    label = label.push(text("current").size(11).color(t.weak));
                }
                if b.is_remote {
                    label = label.push(text("remote").size(11).color(t.weak));
                }
                list = list.push(row_button(label, is_current, true, Message::ModalPick(b.name.clone())));
                shown += 1;
                if shown >= 200 {
                    break;
                }
            }
            if shown == 0 {
                list = list.push(text("no branch matches").size(12).color(t.weak));
            }
            let mut actions = row![
                button("New branch", (!busy).then_some(Message::OpenNewBranch)),
            ]
            .spacing(8);
            if !app.has_origin() {
                actions = actions.push(button("Publish to GitHub", (!busy).then_some(Message::OpenPublish)));
            }
            actions = actions.push(Space::new().width(Length::Fill)).push(button("Close", Some(Message::ModalClose)));
            column![
                input("filter branches", filter, true, Message::ModalValue),
                container(scrollable(list)).max_height(260.0),
                actions,
            ]
            .spacing(10)
            .into()
        }
        Modal::CheckoutConfirm { target } => column![
            text(format!(
                "You have uncommitted changes. Stash them and switch to {target}, or discard them?"
            ))
            .size(13),
            row![
                Space::new().width(Length::Fill),
                button("Cancel", Some(Message::ModalClose)),
                danger_button("Discard and switch", (!busy).then_some(Message::ModalCheckoutForce)),
                primary_button("Stash and switch", (!busy).then_some(Message::ModalCheckoutStash)),
            ]
            .spacing(8),
        ]
        .spacing(12)
        .into(),
        Modal::PublishGithub {
            name,
            description,
            private,
        } => column![
            text("Creates the repository with gh, adds origin and pushes the current branch.").size(12).color(t.weak),
            input("repository name", name, true, Message::ModalValue),
            input("description (optional)", description, false, Message::ModalExtra),
            checkbox(*private).label("private").size(14).text_size(12).on_toggle(Message::ModalCheckbox),
            buttons(
                primary_button("Publish", (!busy && !name.trim().is_empty()).then_some(Message::ModalConfirm)),
                "Cancel"
            ),
        ]
        .spacing(10)
        .into(),
        Modal::Confirm { body, button: label, .. } => column![
            text(body).size(13),
            buttons(danger_button(*label, (!busy).then_some(Message::ModalConfirm)), "Cancel"),
        ]
        .spacing(12)
        .into(),
        Modal::Input { kind, value, extra } => {
            let (hint, hint2) = kind.hints();
            let mut col = column![].spacing(10);
            if let InputKind::Tag { label, .. } = kind {
                col = col.push(text(format!("At {label}")).size(12).color(t.weak));
            }
            if matches!(kind, InputKind::Reword { .. }) {
                col = col.push(
                    iced_widget::TextEditor::new(&app.modal_multiline)
                        .id(widgets::MODAL_INPUT_ID.clone())
                        .on_action(Message::ModalMultiline)
                        .size(13)
                        .padding(6)
                        .height(Length::Fixed(120.0))
                        .style(widgets::text_editor_style),
                );
            } else {
                col = col.push(input(hint, value, true, Message::ModalValue));
            }
            if let Some(h2) = hint2 {
                col = col.push(input(h2, extra, false, Message::ModalExtra));
            }
            let valid = if matches!(kind, InputKind::Reword { .. }) {
                !app.modal_multiline.text().trim().is_empty()
            } else {
                kind.valid(value, extra)
            };
            col = col.push(buttons(primary_button("OK", (!busy && valid).then_some(Message::ModalConfirm)), "Cancel"));
            col.into()
        }
        Modal::Reset { label, .. } => column![
            text(format!("Reset the current branch to {label}")).size(13),
            column![
                row_button(
                    column![text("Soft").size(13), text("keep the index and working tree").size(11).color(t.weak)],
                    false,
                    true,
                    Message::ModalReset(ResetKind::Soft)
                ),
                row_button(
                    column![text("Mixed").size(13), text("keep the working tree, reset the index").size(11).color(t.weak)],
                    false,
                    true,
                    Message::ModalReset(ResetKind::Mixed)
                ),
                row_button(
                    column![text("Hard").size(13), text("discard everything after it").size(11).color(t.error)],
                    false,
                    true,
                    Message::ModalReset(ResetKind::Hard)
                ),
            ]
            .spacing(2),
            row![Space::new().width(Length::Fill), button("Cancel", Some(Message::ModalClose))],
        ]
        .spacing(12)
        .into(),
        Modal::StashOpts {
            message,
            keep_index,
            include_untracked,
        } => column![
            input("message (optional)", message, true, Message::ModalValue),
            checkbox(*keep_index).label("keep the index").size(14).text_size(12).on_toggle(Message::ModalCheckbox),
            checkbox(*include_untracked).label("include untracked files").size(14).text_size(12).on_toggle(Message::ModalCheckbox2),
            buttons(primary_button("Stash", (!busy).then_some(Message::ModalConfirm)), "Cancel"),
        ]
        .spacing(10)
        .into(),
        Modal::StateMenu => {
            let s = &app.snapshot;
            let mut label = format!("A {} is in progress", s.state.label());
            if let Some((done, total)) = s.rebase_progress {
                label.push_str(&format!(" ({done}/{total})"));
            }
            column![
                text(label).size(13),
                text(if s.conflicted.is_empty() {
                    "Resolve conflicts in the working tree, stage them, then continue.".to_owned()
                } else {
                    format!("{} conflicted file(s) still need resolving.", s.conflicted.len())
                })
                .size(12)
                .color(t.weak),
                row![
                    Space::new().width(Length::Fill),
                    button("Cancel", Some(Message::ModalClose)),
                    danger_button("Abort", (!busy).then_some(Message::StateAction(StateAction::Abort))),
                    button("Skip", (!busy).then_some(Message::StateAction(StateAction::Skip))),
                    primary_button("Continue", (!busy && s.conflicted.is_empty()).then_some(Message::StateAction(StateAction::Continue))),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into()
        }
        Modal::Help => column![
            help::view(app),
            row![Space::new().width(Length::Fill), button("Close", Some(Message::ModalClose))],
        ]
        .spacing(10)
        .into(),
        Modal::CloseEditor => {
            let path = app.editor.as_ref().map(|e| e.path.clone()).unwrap_or_default();
            column![
                text(format!("{path} has unsaved changes.")).size(13),
                row![
                    Space::new().width(Length::Fill),
                    button("Cancel", Some(Message::ModalClose)),
                    danger_button("Discard changes", Some(Message::ModalEditorDiscard)),
                    primary_button("Save and close", Some(Message::ModalEditorSave)),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into()
        }
    };
    let width = match modal {
        Modal::BranchPicker { .. } | Modal::Help => 520.0,
        _ => 420.0,
    };
    let dialog = container(
        column![text(title).size(15).color(t.strong), body].spacing(12),
    )
    .padding(16)
    .width(Length::Fixed(width))
    .style(widgets::panel_style);
    let dim = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
                ..Default::default()
            }),
    )
    .on_press(Message::ModalClose);
    iced_widget::stack![
        dim,
        iced_widget::center(dialog).width(Length::Fill).height(Length::Fill)
    ]
    .into()
}
