//! Shared building blocks: pane chrome, buttons, rows, toasts, widget ids.

use std::sync::LazyLock;

use iced_core::widget::Id;
use iced_core::{Alignment, Background, Border, Color, Length, Padding};
use iced_widget::pane_grid::{self, TitleBar};
use iced_widget::button as button_widget;
use iced_widget::{column, container, row, text, Space};

use crate::ui::app::{App, Element, Message};
use crate::ui::theme::{alpha, Theme};

pub static MODAL_INPUT_ID: LazyLock<Id> = LazyLock::new(|| Id::new("modal_input"));
pub static COMMIT_BOX_ID: LazyLock<Id> = LazyLock::new(|| Id::new("commit_box"));
pub static EDITOR_ID: LazyLock<Id> = LazyLock::new(|| Id::new("editor"));
pub static FILTER_ID: LazyLock<Id> = LazyLock::new(|| Id::new("filter"));
pub static DIFF_SEARCH_ID: LazyLock<Id> = LazyLock::new(|| Id::new("diff_search"));

pub const ROW_H: f32 = 24.0;

fn border(color: Color) -> Border {
    Border {
        color,
        width: 1.0,
        radius: 5.0.into(),
    }
}

/// A regular button: filled, small radius, disabled when `on_press` is None.
pub fn button<'a>(label: impl text::IntoFragment<'a>, on_press: Option<Message>) -> Element<'a> {
    let b = button_widget::Button::new(text(label).size(13).wrapping(iced_core::text::Wrapping::None)).padding([4, 10]).style(button_style);
    match on_press {
        Some(m) => b.on_press(m),
        None => b,
    }
    .into()
}

/// A compact button for headers and rows.
pub fn small_button<'a>(label: impl text::IntoFragment<'a>, on_press: Option<Message>) -> Element<'a> {
    let b = button_widget::Button::new(text(label).size(12).wrapping(iced_core::text::Wrapping::None)).padding([2, 7]).style(button_style);
    match on_press {
        Some(m) => b.on_press(m),
        None => b,
    }
    .into()
}

/// A button that reads as the main action of a dialog.
pub fn primary_button<'a>(label: impl text::IntoFragment<'a>, on_press: Option<Message>) -> Element<'a> {
    let b = button_widget::Button::new(text(label).size(13).wrapping(iced_core::text::Wrapping::None))
        .padding([4, 12])
        .style(|theme: &iced_core::Theme, status| {
            let p = theme.extended_palette();
            let base = button_widget::Style {
                background: Some(Background::Color(p.primary.base.color)),
                text_color: p.primary.base.text,
                border: border(Color::TRANSPARENT),
                ..Default::default()
            };
            match status {
                button_widget::Status::Hovered => button_widget::Style {
                    background: Some(Background::Color(p.primary.strong.color)),
                    ..base
                },
                button_widget::Status::Disabled => button_widget::Style {
                    background: Some(Background::Color(alpha(p.primary.base.color, 0.4))),
                    text_color: alpha(p.primary.base.text, 0.6),
                    ..base
                },
                _ => base,
            }
        });
    match on_press {
        Some(m) => b.on_press(m),
        None => b,
    }
    .into()
}

/// A destructive action.
pub fn danger_button<'a>(label: impl text::IntoFragment<'a>, on_press: Option<Message>) -> Element<'a> {
    let b = button_widget::Button::new(text(label).size(13).wrapping(iced_core::text::Wrapping::None))
        .padding([4, 12])
        .style(|theme: &iced_core::Theme, status| {
            let p = theme.extended_palette();
            let base = button_widget::Style {
                background: Some(Background::Color(p.danger.base.color)),
                text_color: p.danger.base.text,
                border: border(Color::TRANSPARENT),
                ..Default::default()
            };
            match status {
                button_widget::Status::Hovered => button_widget::Style {
                    background: Some(Background::Color(p.danger.strong.color)),
                    ..base
                },
                button_widget::Status::Disabled => button_widget::Style {
                    background: Some(Background::Color(alpha(p.danger.base.color, 0.4))),
                    ..base
                },
                _ => base,
            }
        });
    match on_press {
        Some(m) => b.on_press(m),
        None => b,
    }
    .into()
}

pub fn button_style(theme: &iced_core::Theme, status: button_widget::Status) -> button_widget::Style {
    let p = theme.extended_palette();
    let base = button_widget::Style {
        background: Some(Background::Color(p.background.weak.color)),
        text_color: p.background.base.text,
        border: border(p.background.strong.color),
        ..Default::default()
    };
    match status {
        button_widget::Status::Hovered => button_widget::Style {
            background: Some(Background::Color(p.background.strong.color)),
            ..base
        },
        button_widget::Status::Pressed => button_widget::Style {
            background: Some(Background::Color(p.background.strongest.color)),
            ..base
        },
        button_widget::Status::Disabled => button_widget::Style {
            background: Some(Background::Color(alpha(p.background.weak.color, 0.5))),
            text_color: alpha(p.background.base.text, 0.4),
            border: border(alpha(p.background.strong.color, 0.5)),
            ..base
        },
        button_widget::Status::Active => base,
    }
}

/// A list row: full width, highlighted when selected, hover tint otherwise.
pub fn row_button<'a>(content: impl Into<Element<'a>>, selected: bool, focused: bool, on_press: Message) -> Element<'a> {
    button_widget::Button::new(content)
        .width(Length::Fill)
        .padding([2, 6])
        .on_press(on_press)
        .style(move |theme: &iced_core::Theme, status| {
            let p = theme.extended_palette();
            let bg = if selected {
                Some(Background::Color(if focused {
                    alpha(p.primary.base.color, 0.35)
                } else {
                    p.background.strong.color
                }))
            } else if status == button_widget::Status::Hovered {
                Some(Background::Color(p.background.weak.color))
            } else {
                None
            };
            button_widget::Style {
                background: bg,
                text_color: p.background.base.text,
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

/// A plain text line in the weak color.
pub fn weak<'a>(s: impl text::IntoFragment<'a>, theme: &Theme) -> iced_widget::Text<'a, iced_core::Theme, crate::ui::app::Renderer> {
    text(s).size(12).color(theme.weak)
}

/// A colored label pill (refs in the log, status letters).
pub fn pill<'a>(label: impl text::IntoFragment<'a>, color: Color) -> Element<'a> {
    container(text(label).size(11).color(Color::WHITE))
        .padding([1, 6])
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Section header inside the sidebar.
pub fn section<'a>(title: &'a str, trailing: Option<Element<'a>>, theme: &Theme) -> Element<'a> {
    let mut r = row![text(title).size(12).color(theme.weak)].spacing(6).align_y(Alignment::Center);
    if let Some(t) = trailing {
        r = r.push(Space::new().width(Length::Fill)).push(t);
    }
    container(r).padding(Padding::from([6, 6]).top(10)).width(Length::Fill).into()
}

/// The pane frame: title bar with maximize / restore controls, body inside.
pub fn pane<'a>(
    app: &'a App,
    pane: pane_grid::Pane,
    title: &'a str,
    focused: bool,
    maximized: bool,
    body: Element<'a>,
) -> pane_grid::Content<'a, Message, iced_core::Theme, crate::ui::app::Renderer> {
    let t = &app.theme;
    let controls: Element<'a> = if maximized {
        small_button("restore", Some(Message::PaneRestore))
    } else {
        small_button("max", Some(Message::PaneMaximize(pane)))
    };
    let title_color = if focused { t.strong } else { t.weak };
    let bar = TitleBar::new(
        row![text(title).size(12).color(title_color)]
            .align_y(Alignment::Center)
            .padding(Padding::from([2, 6])),
    )
    .controls(controls)
    .padding(2)
    .style(|theme: &iced_core::Theme| {
        let p = theme.extended_palette();
        container::Style {
            background: Some(Background::Color(p.background.weak.color)),
            text_color: Some(p.background.base.text),
            border: Border {
                radius: iced_core::border::Radius::new(6.0).bottom(0.0),
                ..Default::default()
            },
            ..Default::default()
        }
    });
    let accent = t.accent;
    let border_color = t.border;
    pane_grid::Content::new(container(body).width(Length::Fill).height(Length::Fill))
        .title_bar(bar)
        .style(move |theme: &iced_core::Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(Background::Color(p.background.base.color)),
                border: Border {
                    color: if focused { alpha(accent, 0.6) } else { border_color },
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            }
        })
}

pub fn pane_grid_style(theme: &iced_core::Theme) -> pane_grid::Style {
    let p = theme.extended_palette();
    pane_grid::Style {
        hovered_region: pane_grid::Highlight {
            background: Background::Color(alpha(p.primary.base.color, 0.25)),
            border: Border {
                color: p.primary.strong.color,
                width: 2.0,
                radius: 6.0.into(),
            },
        },
        picked_split: pane_grid::Line {
            color: p.primary.strong.color,
            width: 3.0,
        },
        hovered_split: pane_grid::Line {
            color: alpha(p.primary.base.color, 0.7),
            width: 3.0,
        },
    }
}

/// Toasts stacked in the bottom right corner.
pub fn toasts(app: &App) -> Element<'_> {
    let t = &app.theme;
    let mut col = column![].spacing(6).align_x(Alignment::End);
    for toast in app.toasts.iter().rev().take(4) {
        let color = if toast.error { t.error } else { t.ok };
        let bg = t.panel;
        col = col.push(
            container(text(&toast.text).size(12).color(t.strong))
                .padding([6, 12])
                .style(move |_| container::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        color,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }),
        );
    }
    container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::End)
        .padding(Padding::from([12, 12]).bottom(40))
        .into()
}

/// A raised box (dialogs, menus).
pub fn panel_style(theme: &iced_core::Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weak.color)),
        text_color: Some(p.background.base.text),
        border: Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: iced_core::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced_core::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    }
}

/// Text field style that follows the theme's well color.
pub fn text_input_style(theme: &iced_core::Theme, status: iced_widget::text_input::Status) -> iced_widget::text_input::Style {
    use iced_widget::text_input::{Status, Style};
    let p = theme.extended_palette();
    let base = Style {
        background: Background::Color(p.background.weakest.color),
        border: Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: 5.0.into(),
        },
        icon: p.background.weak.text,
        placeholder: alpha(p.background.base.text, 0.45),
        value: p.background.base.text,
        selection: alpha(p.primary.base.color, 0.5),
    };
    match status {
        Status::Focused { .. } => Style {
            border: Border {
                color: p.primary.base.color,
                ..base.border
            },
            ..base
        },
        Status::Disabled => Style {
            value: alpha(p.background.base.text, 0.5),
            ..base
        },
        _ => base,
    }
}

pub fn text_editor_style(theme: &iced_core::Theme, status: iced_widget::text_editor::Status) -> iced_widget::text_editor::Style {
    use iced_widget::text_editor::{Status, Style};
    let p = theme.extended_palette();
    let base = Style {
        background: Background::Color(p.background.weakest.color),
        border: Border {
            color: p.background.strong.color,
            width: 1.0,
            radius: 5.0.into(),
        },
        placeholder: alpha(p.background.base.text, 0.45),
        value: p.background.base.text,
        selection: alpha(p.primary.base.color, 0.5),
    };
    match status {
        Status::Focused { .. } => Style {
            border: Border {
                color: p.primary.base.color,
                ..base.border
            },
            ..base
        },
        _ => base,
    }
}
