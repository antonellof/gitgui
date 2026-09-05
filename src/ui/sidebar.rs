//! Branches, remotes, tags, stashes, each with a right-click menu.

use crate::git::ops::Command;
use crate::git::repo::short_id;
use crate::ui::app::{App, InputKind, Modal, Pane, Selection};
use crate::ui::logo;

/// What a menu item asked for; applied after the lists are drawn.
enum Act {
    Cmd(Command),
    Modal(Modal),
    Confirm {
        title: &'static str,
        body: String,
        button: &'static str,
        cmd: Command,
    },
    Input {
        kind: InputKind,
        value: String,
    },
    Copy(String),
    PullRequest(String),
    Switch(String),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let focused = app.focus == Pane::Sidebar;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        logo::show(ui, &app.theme);
        ui.heading("gitgui");
        if focused {
            ui.weak("*");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("?").on_hover_text("Keyboard shortcuts").clicked() {
                app.open_help();
            }
        });
    });
    ui.separator();
    let mut acts: Vec<Act> = Vec::new();
    let mut clicked: Option<(String, git2::Oid)> = None;
    egui::ScrollArea::vertical()
        .id_salt("sidebar_scroll")
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let snapshot = app.snapshot.clone();
            let busy = app.busy > 0;
            let current = snapshot
                .head
                .as_ref()
                .and_then(|h| h.branch_name.clone());
            let has_web = app.web_remote().is_some();

            egui::CollapsingHeader::new("Local")
                .default_open(true)
                .show(ui, |ui| {
                    for b in snapshot.branches.iter().filter(|b| !b.is_remote) {
                        let label = if b.is_head {
                            format!("* {}", b.name)
                        } else {
                            format!("  {}", b.name)
                        };
                        let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
                        let mut text = egui::RichText::new(label);
                        if b.is_head {
                            text = text.strong();
                        }
                        let resp = ui.selectable_label(selected, text);
                        let resp = match (&b.upstream, b.ahead, b.behind) {
                            (Some(up), a, bh) if a > 0 || bh > 0 => {
                                resp.on_hover_text(format!("{a} ahead, {bh} behind {up}"))
                            }
                            (Some(up), _, _) => resp.on_hover_text(format!("tracks {up}")),
                            (None, _, _) => resp.on_hover_text("no upstream"),
                        };
                        if resp.clicked() {
                            clicked = Some((b.name.clone(), b.oid));
                        }
                        if resp.double_clicked() && !b.is_head && !busy {
                            acts.push(Act::Switch(b.name.clone()));
                        }
                        resp.context_menu(|ui| {
                            let name = b.name.clone();
                            let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, tip: &str, act: Act| {
                                let r = ui.add_enabled(enabled && !busy, egui::Button::new(label));
                                let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
                                if r.clicked() {
                                    acts.push(act);
                                    ui.close();
                                }
                            };
                            item(ui, !b.is_head, "Checkout", "Enter", Act::Switch(name.clone()));
                            item(
                                ui,
                                true,
                                "New branch from here",
                                "",
                                Act::Modal(Modal::NewBranch {
                                    name: String::new(),
                                    from: b.oid,
                                    from_label: name.clone(),
                                    checkout: true,
                                }),
                            );
                            item(
                                ui,
                                true,
                                "Rename",
                                "",
                                Act::Input {
                                    kind: InputKind::RenameBranch { old: name.clone() },
                                    value: name.clone(),
                                },
                            );
                            item(ui, !b.is_head, "Delete", "", Act::Modal(Modal::DeleteBranch(name.clone())));
                            ui.separator();
                            let cur = current.clone().unwrap_or_else(|| "HEAD".into());
                            item(
                                ui,
                                !b.is_head && current.is_some(),
                                &format!("Merge into {cur}"),
                                "",
                                Act::Confirm {
                                    title: "Merge",
                                    body: format!("Merge {name} into {cur}?"),
                                    button: "Merge",
                                    cmd: Command::Merge(name.clone()),
                                },
                            );
                            item(
                                ui,
                                !b.is_head && current.is_some(),
                                &format!("Rebase {cur} onto this"),
                                "git rebase",
                                Act::Confirm {
                                    title: "Rebase",
                                    body: format!("Rebase {cur} onto {name}? Conflicts stop the rebase for you to resolve."),
                                    button: "Rebase",
                                    cmd: Command::Rebase(name.clone()),
                                },
                            );
                            item(
                                ui,
                                b.upstream.is_some() && b.behind > 0 && b.ahead == 0,
                                "Fast-forward from upstream",
                                "",
                                Act::Cmd(Command::FastForward(name.clone())),
                            );
                            ui.separator();
                            let default_up = b
                                .upstream
                                .clone()
                                .unwrap_or_else(|| format!("origin/{name}"));
                            item(
                                ui,
                                true,
                                "Set upstream",
                                "remote branch this one tracks",
                                Act::Input {
                                    kind: InputKind::SetUpstream { branch: name.clone() },
                                    value: default_up,
                                },
                            );
                            item(
                                ui,
                                b.upstream.is_some(),
                                "Unset upstream",
                                "",
                                Act::Cmd(Command::SetUpstream {
                                    branch: name.clone(),
                                    upstream: None,
                                }),
                            );
                            item(
                                ui,
                                has_web,
                                "Open pull request",
                                "in the browser",
                                Act::PullRequest(name.clone()),
                            );
                            ui.separator();
                            item(ui, true, "Copy name", "", Act::Copy(name.clone()));
                        });
                    }
                });

            let remote_count = snapshot.branches.iter().filter(|b| b.is_remote).count();
            egui::CollapsingHeader::new(format!("Remote ({remote_count})"))
                .default_open(remote_count <= 30)
                .show(ui, |ui| {
                    for r in &snapshot.remotes {
                        let url = snapshot
                            .remote_urls
                            .iter()
                            .find(|(n, _)| n == r)
                            .map(|(_, u)| u.clone())
                            .unwrap_or_default();
                        let resp = ui
                            .selectable_label(false, egui::RichText::new(format!("  {r}")).weak())
                            .on_hover_text(&url);
                        resp.context_menu(|ui| {
                            let name = r.clone();
                            let mut item = |ui: &mut egui::Ui, label: &str, act: Act| {
                                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                    acts.push(act);
                                    ui.close();
                                }
                            };
                            item(ui, "Fetch", Act::Cmd(Command::FetchRemote(name.clone())));
                            item(
                                ui,
                                "Edit URL",
                                Act::Input {
                                    kind: InputKind::RemoteUrl { name: name.clone() },
                                    value: url.clone(),
                                },
                            );
                            item(
                                ui,
                                "Rename",
                                Act::Input {
                                    kind: InputKind::RemoteRename { old: name.clone() },
                                    value: name.clone(),
                                },
                            );
                            item(
                                ui,
                                "Remove",
                                Act::Confirm {
                                    title: "Remove remote",
                                    body: format!("Remove remote {name}? Local branches are not affected."),
                                    button: "Remove",
                                    cmd: Command::RemoteRemove(name.clone()),
                                },
                            );
                            item(ui, "Copy URL", Act::Copy(url.clone()));
                        });
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Add remote").small())
                        .clicked()
                    {
                        acts.push(Act::Input {
                            kind: InputKind::RemoteAdd,
                            value: if snapshot.remotes.is_empty() {
                                "origin".into()
                            } else {
                                String::new()
                            },
                        });
                    }
                    for b in snapshot.branches.iter().filter(|b| b.is_remote) {
                        let selected = app.sidebar_selected.as_deref() == Some(b.name.as_str());
                        let resp = ui.selectable_label(selected, format!("  {}", b.name));
                        if resp.clicked() {
                            clicked = Some((b.name.clone(), b.oid));
                        }
                        if resp.double_clicked() && !busy {
                            acts.push(Act::Switch(b.name.clone()));
                        }
                        resp.context_menu(|ui| {
                            let name = b.name.clone();
                            let (remote, short) = name
                                .split_once('/')
                                .map(|(r, s)| (r.to_owned(), s.to_owned()))
                                .unwrap_or((String::new(), name.clone()));
                            let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, act: Act| {
                                if ui.add_enabled(enabled && !busy, egui::Button::new(label)).clicked() {
                                    acts.push(act);
                                    ui.close();
                                }
                            };
                            item(ui, true, "Checkout (track)", Act::Switch(name.clone()));
                            item(ui, true, "Checkout (detached HEAD)", Act::Cmd(Command::CheckoutDetached(b.oid)));
                            item(
                                ui,
                                true,
                                "New branch from here",
                                Act::Modal(Modal::NewBranch {
                                    name: String::new(),
                                    from: b.oid,
                                    from_label: name.clone(),
                                    checkout: true,
                                }),
                            );
                            ui.separator();
                            let cur = current.clone().unwrap_or_else(|| "HEAD".into());
                            item(
                                ui,
                                current.is_some(),
                                &format!("Merge into {cur}"),
                                Act::Confirm {
                                    title: "Merge",
                                    body: format!("Merge {name} into {cur}?"),
                                    button: "Merge",
                                    cmd: Command::Merge(name.clone()),
                                },
                            );
                            item(
                                ui,
                                current.is_some(),
                                &format!("Rebase {cur} onto this"),
                                Act::Confirm {
                                    title: "Rebase",
                                    body: format!("Rebase {cur} onto {name}? Conflicts stop the rebase for you to resolve."),
                                    button: "Rebase",
                                    cmd: Command::Rebase(name.clone()),
                                },
                            );
                            item(
                                ui,
                                current.is_some(),
                                &format!("Set as upstream of {cur}"),
                                Act::Cmd(Command::SetUpstream {
                                    branch: cur.clone(),
                                    upstream: Some(name.clone()),
                                }),
                            );
                            ui.separator();
                            item(
                                ui,
                                !remote.is_empty(),
                                "Delete on remote",
                                Act::Confirm {
                                    title: "Delete remote branch",
                                    body: format!("Delete {short} on {remote}? This runs git push {remote} --delete {short}."),
                                    button: "Delete",
                                    cmd: Command::DeleteRemoteBranch {
                                        remote: remote.clone(),
                                        branch: short.clone(),
                                    },
                                },
                            );
                            item(ui, true, "Copy name", Act::Copy(name.clone()));
                        });
                    }
                    if snapshot.branches.iter().all(|b| !b.is_remote) {
                        ui.weak("  none");
                    }
                });

            egui::CollapsingHeader::new("Tags")
                .default_open(snapshot.tags.len() <= 20)
                .show(ui, |ui| {
                    for t in &snapshot.tags {
                        let selected = app.sidebar_selected.as_deref() == Some(t.name.as_str());
                        let resp = ui.selectable_label(selected, format!("  {}", t.name));
                        if resp.clicked() {
                            clicked = Some((t.name.clone(), t.oid));
                        }
                        resp.context_menu(|ui| {
                            let name = t.name.clone();
                            let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, act: Act| {
                                if ui.add_enabled(enabled && !busy, egui::Button::new(label)).clicked() {
                                    acts.push(act);
                                    ui.close();
                                }
                            };
                            item(ui, true, "Checkout (detached HEAD)", Act::Cmd(Command::CheckoutDetached(t.oid)));
                            item(
                                ui,
                                true,
                                "New branch from here",
                                Act::Modal(Modal::NewBranch {
                                    name: String::new(),
                                    from: t.oid,
                                    from_label: name.clone(),
                                    checkout: true,
                                }),
                            );
                            for r in &snapshot.remotes {
                                item(
                                    ui,
                                    true,
                                    &format!("Push to {r}"),
                                    Act::Cmd(Command::PushTag {
                                        remote: r.clone(),
                                        tag: name.clone(),
                                    }),
                                );
                            }
                            item(
                                ui,
                                true,
                                "Delete",
                                Act::Confirm {
                                    title: "Delete tag",
                                    body: format!("Delete local tag {name}?"),
                                    button: "Delete",
                                    cmd: Command::DeleteTag(name.clone()),
                                },
                            );
                            item(ui, true, "Copy name", Act::Copy(name.clone()));
                        });
                    }
                    if let Some(oid) = snapshot.head.as_ref().and_then(|h| h.oid) {
                        if ui
                            .add_enabled(!busy, egui::Button::new("New tag at HEAD").small())
                            .clicked()
                        {
                            acts.push(Act::Input {
                                kind: InputKind::Tag {
                                    oid,
                                    label: format!("HEAD ({})", short_id(oid)),
                                },
                                value: String::new(),
                            });
                        }
                    }
                    if snapshot.tags.is_empty() {
                        ui.weak("  none");
                    }
                });

            egui::CollapsingHeader::new("Stashes")
                .default_open(true)
                .show(ui, |ui| {
                    for s in &snapshot.stashes {
                        let selected = app.sidebar_selected.as_deref() == Some(s.message.as_str());
                        let resp =
                            ui.selectable_label(selected, format!("  {}: {}", s.index, s.message));
                        if resp.clicked() {
                            clicked = Some((s.message.clone(), s.oid));
                        }
                        resp.context_menu(|ui| {
                            let mut item = |ui: &mut egui::Ui, label: &str, tip: &str, act: Act| {
                                let r = ui.add_enabled(!busy, egui::Button::new(label));
                                let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
                                if r.clicked() {
                                    acts.push(act);
                                    ui.close();
                                }
                            };
                            item(ui, "Apply", "keep the stash", Act::Cmd(Command::StashApply(s.index)));
                            item(ui, "Pop", "apply and drop", Act::Cmd(Command::StashPop(s.index)));
                            item(
                                ui,
                                "New branch from stash",
                                "check out the stash base, create the branch, apply and drop",
                                Act::Input {
                                    kind: InputKind::BranchFromStash { index: s.index },
                                    value: String::new(),
                                },
                            );
                            item(ui, "Drop", "", Act::Modal(Modal::DropStash(s.index)));
                        });
                    }
                    if ui
                        .add_enabled(
                            !busy && snapshot.is_dirty(),
                            egui::Button::new("Stash changes").small(),
                        )
                        .on_hover_text("Shift+S")
                        .clicked()
                    {
                        acts.push(Act::Modal(Modal::StashOpts {
                            message: String::new(),
                            keep_index: false,
                            include_untracked: true,
                        }));
                    }
                    if snapshot.stashes.is_empty() {
                        ui.weak("  none");
                    }
                });

            crate::ui::tree::show(app, ui);
        });

    if let Some((name, oid)) = clicked {
        app.sidebar_selected = Some(name);
        app.focus = Pane::Sidebar;
        if let Some(idx) = app.snapshot.commits.iter().position(|c| c.oid == oid) {
            app.select(Selection::Commit(idx));
            app.scroll_to_selection = true;
        } else {
            app.toast(format!("{} is not in the loaded log", short_id(oid)), false);
        }
    }
    let ctx = ui.ctx().clone();
    for act in acts {
        match act {
            Act::Cmd(c) => app.run(c),
            Act::Modal(m) => {
                if app.busy == 0 {
                    app.modal = Some(m);
                }
            }
            Act::Confirm {
                title,
                body,
                button,
                cmd,
            } => app.confirm(title, body, button, cmd),
            Act::Input { kind, value } => app.input(kind, value, String::new()),
            Act::Copy(text) => {
                ctx.copy_text(text);
                app.toast("copied", false);
            }
            Act::PullRequest(branch) => app.open_pull_request(&branch),
            Act::Switch(name) => app.try_switch_branch(name),
        }
    }
}
