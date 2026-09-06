//! Right-click menus, drawn as a floating panel at the pointer. Any item
//! closes the menu through `Message::MenuPick`; a click elsewhere closes it.

use iced_core::{Background, Color, Length, Padding};
use iced_widget::{button, column, container, mouse_area, rule, text, Space};

use crate::git::actions::ConflictSide;
use crate::git::ops::Command;
use crate::git::rebase::TodoAction;
use crate::ui::app::{App, CommitAction, Element, InputKind, Menu, MenuKind, Message, Modal};
use crate::ui::widgets;

/// A menu entry: label, tip, message (None = disabled).
enum Item {
    Entry(String, &'static str, Option<Message>),
    Sep,
}

fn entry(label: impl Into<String>, tip: &'static str, msg: Option<Message>) -> Item {
    Item::Entry(label.into(), tip, msg)
}

fn items(app: &App, kind: &MenuKind) -> Vec<Item> {
    let busy = app.busy > 0;
    let s = &app.snapshot;
    let on = |m: Message| if busy { None } else { Some(m) };
    let current = s.head.as_ref().and_then(|h| h.branch_name.clone());
    let cur = current.clone().unwrap_or_else(|| "HEAD".into());
    match kind {
        MenuKind::Commit(idx) => {
            let idx = *idx;
            let head = s
                .head
                .as_ref()
                .and_then(|h| h.oid)
                .is_some_and(|h| s.commits.get(idx).is_some_and(|c| c.oid == h));
            let info = app.rewrite_info(idx);
            let rewrite = info.is_some();
            let has_older = info.is_some_and(|i| i.has_older);
            let has_newer = info.is_some_and(|i| !i.is_head);
            let act = |a: CommitAction, enabled: bool| if enabled && !busy { Some(Message::CommitAction(idx, a)) } else { None };
            vec![
                entry("New branch here", "n", act(CommitAction::NewBranch, true)),
                entry("Tag here", "Shift+T", act(CommitAction::Tag, true)),
                entry("Check out (detached HEAD)", "", act(CommitAction::CheckoutDetached, true)),
                Item::Sep,
                entry("Cherry-pick onto HEAD", "Shift+C", act(CommitAction::CherryPick, !head)),
                entry("Revert", "t", act(CommitAction::Revert, true)),
                entry("Reset current branch here", "g", act(CommitAction::Reset, true)),
                Item::Sep,
                entry("Reword", "Shift+R", act(CommitAction::Reword, rewrite)),
                entry("Squash into commit below", "keeps both messages", act(CommitAction::Rewrite(TodoAction::Squash), rewrite && has_older)),
                entry("Fixup into commit below", "discards this message", act(CommitAction::Rewrite(TodoAction::Fixup), rewrite && has_older)),
                entry("Drop", "d", act(CommitAction::Rewrite(TodoAction::Drop), rewrite && !head)),
                entry("Move up", "Shift+K", act(CommitAction::Rewrite(TodoAction::MoveUp), rewrite && has_newer)),
                entry("Move down", "Shift+J", act(CommitAction::Rewrite(TodoAction::MoveDown), rewrite && has_older)),
                entry("Edit (stop the rebase here)", "continue with m", act(CommitAction::Rewrite(TodoAction::Edit), rewrite && !head)),
                Item::Sep,
                entry("Create fixup commit for this", "commits the staged changes as fixup!", act(CommitAction::CreateFixup, !s.staged.is_empty())),
                entry("Apply fixup commits above", "rebase --autosquash", act(CommitAction::Autosquash, rewrite)),
                Item::Sep,
                entry("Copy hash", "y", Some(Message::CommitAction(idx, CommitAction::CopyHash))),
                entry("Copy message", "", Some(Message::CommitAction(idx, CommitAction::CopyMessage))),
                entry("Open in browser", "o", if app.web_remote().is_some() { Some(Message::CommitAction(idx, CommitAction::OpenBrowser)) } else { None }),
            ]
        }
        MenuKind::Branch(name) => {
            let b = s.branches.iter().find(|b| &b.name == name);
            let is_head = b.is_some_and(|b| b.is_head);
            let oid = b.map(|b| b.oid);
            let upstream = b.and_then(|b| b.upstream.clone());
            let can_ff = b.is_some_and(|b| b.upstream.is_some() && b.behind > 0 && b.ahead == 0);
            let n = name.clone();
            let mut v = vec![
                entry("Checkout", "Enter", if is_head { None } else { on(Message::Switch(n.clone())) }),
            ];
            if let Some(oid) = oid {
                v.push(entry(
                    "New branch from here",
                    "",
                    on(Message::Modal(Modal::NewBranch {
                        name: String::new(),
                        from: oid,
                        from_label: n.clone(),
                        checkout: true,
                    })),
                ));
            }
            v.push(entry("Rename", "", on(Message::Input(InputKind::RenameBranch { old: n.clone() }, n.clone(), String::new()))));
            v.push(entry("Delete", "", if is_head { None } else { on(Message::Modal(Modal::DeleteBranch(n.clone()))) }));
            v.push(Item::Sep);
            v.push(entry(
                format!("Merge into {cur}"),
                "",
                if is_head || current.is_none() {
                    None
                } else {
                    on(Message::Confirm("Merge", format!("Merge {n} into {cur}?"), "Merge", Command::Merge(n.clone())))
                },
            ));
            v.push(entry(
                format!("Rebase {cur} onto this"),
                "git rebase",
                if is_head || current.is_none() {
                    None
                } else {
                    on(Message::Confirm(
                        "Rebase",
                        format!("Rebase {cur} onto {n}? Conflicts stop the rebase for you to resolve."),
                        "Rebase",
                        Command::Rebase(n.clone()),
                    ))
                },
            ));
            v.push(entry("Fast-forward from upstream", "", if can_ff { on(Message::Run(Command::FastForward(n.clone()))) } else { None }));
            v.push(Item::Sep);
            let default_up = upstream.clone().unwrap_or_else(|| format!("origin/{n}"));
            v.push(entry(
                "Set upstream",
                "remote branch this one tracks",
                on(Message::Input(InputKind::SetUpstream { branch: n.clone() }, default_up, String::new())),
            ));
            v.push(entry(
                "Unset upstream",
                "",
                if upstream.is_some() {
                    on(Message::Run(Command::SetUpstream {
                        branch: n.clone(),
                        upstream: None,
                    }))
                } else {
                    None
                },
            ));
            v.push(entry("Open pull request", "in the browser", if app.web_remote().is_some() { Some(Message::PullRequest(n.clone())) } else { None }));
            v.push(Item::Sep);
            v.push(entry("Copy name", "", Some(Message::Copy(n))));
            v
        }
        MenuKind::RemoteBranch(name) => {
            let b = s.branches.iter().find(|b| &b.name == name);
            let oid = b.map(|b| b.oid);
            let n = name.clone();
            let (remote, short) = n
                .split_once('/')
                .map(|(r, s)| (r.to_owned(), s.to_owned()))
                .unwrap_or((String::new(), n.clone()));
            let mut v = vec![entry("Checkout (track)", "", on(Message::Switch(n.clone())))];
            if let Some(oid) = oid {
                v.push(entry("Checkout (detached HEAD)", "", on(Message::CheckoutDetached(oid))));
                v.push(entry(
                    "New branch from here",
                    "",
                    on(Message::Modal(Modal::NewBranch {
                        name: String::new(),
                        from: oid,
                        from_label: n.clone(),
                        checkout: true,
                    })),
                ));
            }
            v.push(Item::Sep);
            v.push(entry(
                format!("Merge into {cur}"),
                "",
                if current.is_none() {
                    None
                } else {
                    on(Message::Confirm("Merge", format!("Merge {n} into {cur}?"), "Merge", Command::Merge(n.clone())))
                },
            ));
            v.push(entry(
                format!("Rebase {cur} onto this"),
                "",
                if current.is_none() {
                    None
                } else {
                    on(Message::Confirm(
                        "Rebase",
                        format!("Rebase {cur} onto {n}? Conflicts stop the rebase for you to resolve."),
                        "Rebase",
                        Command::Rebase(n.clone()),
                    ))
                },
            ));
            v.push(entry(
                format!("Set as upstream of {cur}"),
                "",
                if current.is_none() {
                    None
                } else {
                    on(Message::Run(Command::SetUpstream {
                        branch: cur.clone(),
                        upstream: Some(n.clone()),
                    }))
                },
            ));
            v.push(Item::Sep);
            v.push(entry(
                "Delete on remote",
                "",
                if remote.is_empty() {
                    None
                } else {
                    on(Message::Confirm(
                        "Delete remote branch",
                        format!("Delete {short} on {remote}? This runs git push {remote} --delete {short}."),
                        "Delete",
                        Command::DeleteRemoteBranch {
                            remote: remote.clone(),
                            branch: short.clone(),
                        },
                    ))
                },
            ));
            v.push(entry("Copy name", "", Some(Message::Copy(n))));
            v
        }
        MenuKind::Remote(name) => {
            let url = s
                .remote_urls
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, u)| u.clone())
                .unwrap_or_default();
            let n = name.clone();
            vec![
                entry("Fetch", "", on(Message::Run(Command::FetchRemote(n.clone())))),
                entry("Edit URL", "", on(Message::Input(InputKind::RemoteUrl { name: n.clone() }, url.clone(), String::new()))),
                entry("Rename", "", on(Message::Input(InputKind::RemoteRename { old: n.clone() }, n.clone(), String::new()))),
                entry(
                    "Remove",
                    "",
                    on(Message::Confirm(
                        "Remove remote",
                        format!("Remove remote {n}? Local branches are not affected."),
                        "Remove",
                        Command::RemoteRemove(n.clone()),
                    )),
                ),
                entry("Copy URL", "", Some(Message::Copy(url))),
            ]
        }
        MenuKind::Tag(name) => {
            let tag = s.tags.iter().find(|t| &t.name == name);
            let n = name.clone();
            let mut v = Vec::new();
            if let Some(tag) = tag {
                v.push(entry("Checkout (detached HEAD)", "", on(Message::CheckoutDetached(tag.oid))));
                v.push(entry(
                    "New branch from here",
                    "",
                    on(Message::Modal(Modal::NewBranch {
                        name: String::new(),
                        from: tag.oid,
                        from_label: n.clone(),
                        checkout: true,
                    })),
                ));
            }
            for r in &s.remotes {
                v.push(entry(
                    format!("Push to {r}"),
                    "",
                    on(Message::Run(Command::PushTag {
                        remote: r.clone(),
                        tag: n.clone(),
                    })),
                ));
            }
            v.push(entry(
                "Delete",
                "",
                on(Message::Confirm("Delete tag", format!("Delete local tag {n}?"), "Delete", Command::DeleteTag(n.clone()))),
            ));
            v.push(entry("Copy name", "", Some(Message::Copy(n))));
            v
        }
        MenuKind::Stash(i) => {
            let i = *i;
            vec![
                entry("Apply", "keep the stash", on(Message::Run(Command::StashApply(i)))),
                entry("Pop", "apply and drop", on(Message::Run(Command::StashPop(i)))),
                entry(
                    "New branch from stash",
                    "check out the stash base, create the branch, apply and drop",
                    on(Message::Input(InputKind::BranchFromStash { index: i }, String::new(), String::new())),
                ),
                entry("Drop", "", on(Message::Modal(Modal::DropStash(i)))),
            ]
        }
        MenuKind::File {
            path,
            staged,
            conflicted,
            untracked,
        } => {
            let p = path.clone();
            let mut v = Vec::new();
            if *conflicted {
                v.push(entry("Resolve…", "three-way merge tool", on(Message::MergeOpen(p.clone()))));
                v.push(entry("Use ours", "keep the version of the branch you are on", on(Message::Resolve(p.clone(), ConflictSide::Ours))));
                v.push(entry("Use theirs", "keep the incoming version", on(Message::Resolve(p.clone(), ConflictSide::Theirs))));
                v.push(entry("Mark resolved", "stage the file as it is", on(Message::Run(Command::Stage(vec![p.clone()])))));
                v.push(Item::Sep);
            } else if *staged {
                v.push(entry("Unstage", "u", on(Message::Run(Command::Unstage(vec![p.clone()])))));
            } else {
                v.push(entry("Stage", "s", on(Message::Run(Command::Stage(vec![p.clone()])))));
                v.push(entry("Discard changes", "d, asks for confirmation", on(Message::Discard(vec![p.clone()]))));
                if *untracked {
                    v.push(entry("Add to .gitignore", "i", on(Message::Ignore(p.clone()))));
                }
            }
            v.push(Item::Sep);
            v.push(entry("Edit", "e, built-in editor", Some(Message::Edit)));
            v.push(entry("Open in $EDITOR", "Shift+E, new terminal split", Some(Message::EditorExternal)));
            if crate::split::is_cmux() {
                v.push(entry("Preview in cmux", "Shift+O", Some(Message::EditorPreview)));
            }
            v.push(entry("Copy path", "", Some(Message::Copy(p))));
            v
        }
        MenuKind::TreeFile(path) => {
            let p = path.clone();
            let changed = s.unstaged.iter().chain(s.staged.iter()).chain(s.conflicted.iter()).any(|f| &f.path == path);
            let unstaged = s.unstaged.iter().any(|f| &f.path == path);
            let mut v = vec![
                entry("Edit", "e, built-in editor", Some(Message::TreeOpen(p.clone()))),
                entry("Open in $EDITOR", "Shift+E, new terminal split", Some(Message::EditorExternal)),
            ];
            if crate::split::is_cmux() {
                v.push(entry("Preview in cmux", "Shift+O", Some(Message::EditorPreview)));
            }
            if changed {
                v.push(Item::Sep);
                v.push(entry("Show changes", "select the file in the working tree lists", Some(Message::ShowChanges(p.clone()))));
                if unstaged {
                    v.push(entry("Stage", "s", on(Message::Run(Command::Stage(vec![p.clone()])))));
                }
            }
            v.push(Item::Sep);
            v.push(entry("Copy path", "", Some(Message::Copy(p))));
            v
        }
        MenuKind::TreeDir(path) => {
            let open = app.tree_open.contains(path);
            let p = path.clone();
            vec![
                entry(if open { "Collapse" } else { "Expand" }, "", Some(Message::TreeToggle(p.clone()))),
                entry("Refresh", "", Some(Message::TreeRequest(p.clone()))),
                entry("Copy path", "", Some(Message::Copy(p))),
            ]
        }
    }
}

pub fn view<'a>(app: &'a App, menu: &'a Menu) -> Element<'a> {
    let t = &app.theme;
    let entries = items(app, &menu.kind);
    // Estimated height, to keep the menu inside the window.
    let est: f32 = entries
        .iter()
        .map(|i| match i {
            Item::Sep => 8.0,
            Item::Entry(..) => 27.0,
        })
        .sum::<f32>()
        + 10.0;
    let mut col = column![].spacing(1);
    for item in entries {
        match item {
            Item::Sep => {
                col = col.push(container(rule::horizontal(1)).padding([3, 4]));
            }
            Item::Entry(label, tip, msg) => {
                let enabled = msg.is_some();
                let mut line = iced_widget::row![text(label).size(13).wrapping(iced_core::text::Wrapping::None)]
                    .spacing(10)
                    .align_y(iced_core::Alignment::Center);
                if !tip.is_empty() {
                    line = line.push(Space::new().width(Length::Fill));
                    line = line.push(text(tip).size(11).color(t.weak).wrapping(iced_core::text::Wrapping::None));
                }
                let mut b = button(line)
                    .width(Length::Fill)
                    .padding([3, 10])
                    .style(move |theme: &iced_core::Theme, status| {
                        let p = theme.extended_palette();
                        let bg = if status == button::Status::Hovered && enabled {
                            Some(Background::Color(p.background.strong.color))
                        } else {
                            None
                        };
                        button::Style {
                            background: bg,
                            text_color: if enabled {
                                p.background.base.text
                            } else {
                                crate::ui::theme::alpha(p.background.base.text, 0.4)
                            },
                            border: iced_core::Border {
                                radius: 4.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    });
                if let Some(m) = msg {
                    b = b.on_press(Message::MenuPick(Box::new(m)));
                }
                col = col.push(b);
            }
        }
    }
    const MENU_W: f32 = 340.0;
    let panel = container(col)
        .padding(4)
        .width(Length::Fixed(MENU_W))
        .style(widgets::panel_style);
    let x = menu.at.x.min((app.window.width - MENU_W - 4.0).max(0.0)).max(0.0);
    let y = menu.at.y.min((app.window.height - est - 4.0).max(0.0)).max(0.0);
    let positioned = container(panel)
        .padding(Padding {
            top: y,
            left: x,
            right: 0.0,
            bottom: 0.0,
        })
        .width(Length::Fill)
        .height(Length::Fill);
    let catch = mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            }),
    )
    .on_press(Message::MenuClose)
    .on_right_press(Message::MenuClose);
    iced_widget::stack![catch, positioned].into()
}
