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

pub const PANE_SPACING: f32 = 4.0;
pub const PANE_MIN: f32 = 80.0;
/// Height of a pane title bar (padding 2 + row padding 2 + a 12 pt line).
pub const TITLE_H: f32 = 26.0;

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

/// Section header inside the sidebar: a disclosure arrow, the title, and
/// an optional trailing button. Clicking the title toggles the section.
pub fn section<'a>(title: &'static str, collapsed: bool, trailing: Option<Element<'a>>, theme: &Theme) -> Element<'a> {
    let arrow = if collapsed { "▸" } else { "▾" };
    let head = button_widget::Button::new(
        row![
            text(arrow).size(11).color(theme.weak),
            text(title).size(12).color(theme.weak),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([2, 4])
    .on_press(Message::SectionToggle(title))
    .style(|theme: &iced_core::Theme, status| {
        let p = theme.extended_palette();
        button_widget::Style {
            background: (status == button_widget::Status::Hovered)
                .then_some(Background::Color(p.background.weak.color)),
            text_color: p.background.base.text,
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    });
    let mut r = row![head].spacing(6).align_y(Alignment::Center);
    if let Some(t) = trailing {
        r = r.push(Space::new().width(Length::Fill)).push(t);
    }
    container(r).padding(Padding::from([4, 2]).top(8)).width(Length::Fill).into()
}

/// The pane frame: title bar with maximize / restore controls, body inside.
pub fn pane<'a>(
    app: &'a App,
    pane: pane_grid::Pane,
    title: &'a str,
    focused: bool,
    maximized: bool,
    hovered: bool,
    body: Element<'a>,
) -> pane_grid::Content<'a, Message, iced_core::Theme, crate::ui::app::Renderer> {
    let t = &app.theme;
    let controls: Element<'a> = if maximized {
        icon_button(Icon::Restore, t.text, Message::PaneRestore)
    } else {
        icon_button(Icon::Maximize, t.text, Message::PaneMaximize(pane))
    };
    let title_color = if focused || hovered { t.strong } else { t.weak };
    // The grip and the hint say "drag me"; the pointer turns into a hand.
    let grip_color = if hovered { t.accent } else { alpha(t.weak, 0.6) };
    // Keep the title row the same width hovered or not: a wider title row
    // makes the title bar decide it does not fit beside the controls and
    // drop the title altogether.
    let head = row![
        text("⋮⋮").size(12).color(grip_color),
        text(title).size(12).color(title_color).wrapping(iced_core::text::Wrapping::None),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .padding(Padding::from([2, 6]));
    let accent = t.accent;
    let bar = TitleBar::new(head)
        .controls(controls)
        .always_show_controls()
        .padding(2)
        .style(move |theme: &iced_core::Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(Background::Color(if hovered {
                    alpha(accent, 0.28)
                } else {
                    p.background.weak.color
                })),
                text_color: Some(p.background.base.text),
                border: Border {
                    radius: iced_core::border::Radius::new(6.0).bottom(0.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
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

/// Draws its child inside a fresh render layer. tiny-skia draws layers in
/// creation order, and the custom widgets create per-text layers, so an
/// overlay drawn later into the root layer would still sit under them.
pub struct Layered<'a> {
    child: Element<'a>,
}

pub fn layered<'a>(child: impl Into<Element<'a>>) -> Element<'a> {
    iced_core::Element::new(Layered { child: child.into() })
}

impl<'a> iced_core::Widget<Message, iced_core::Theme, crate::ui::app::Renderer> for Layered<'a> {
    fn size(&self) -> iced_core::Size<Length> {
        self.child.as_widget().size()
    }

    fn size_hint(&self) -> iced_core::Size<Length> {
        self.child.as_widget().size_hint()
    }

    fn tag(&self) -> iced_core::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced_core::widget::tree::State {
        self.child.as_widget().state()
    }

    fn children(&self) -> Vec<iced_core::widget::Tree> {
        self.child.as_widget().children()
    }

    fn diff(&self, tree: &mut iced_core::widget::Tree) {
        self.child.as_widget().diff(tree);
    }

    fn layout(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        renderer: &crate::ui::app::Renderer,
        limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        self.child.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        renderer: &crate::ui::app::Renderer,
        operation: &mut dyn iced_core::widget::Operation,
    ) {
        self.child.as_widget_mut().operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        event: &iced_core::Event,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        renderer: &crate::ui::app::Renderer,
        clipboard: &mut dyn iced_core::Clipboard,
        shell: &mut iced_core::Shell<'_, Message>,
        viewport: &iced_core::Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        viewport: &iced_core::Rectangle,
        renderer: &crate::ui::app::Renderer,
    ) -> iced_core::mouse::Interaction {
        self.child.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &iced_core::widget::Tree,
        renderer: &mut crate::ui::app::Renderer,
        theme: &iced_core::Theme,
        style: &iced_core::renderer::Style,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        viewport: &iced_core::Rectangle,
    ) {
        use iced_core::Renderer as _;
        renderer.with_layer(*viewport, |renderer| {
            self.child.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced_core::widget::Tree,
        layout: iced_core::Layout<'b>,
        renderer: &crate::ui::app::Renderer,
        viewport: &iced_core::Rectangle,
        translation: iced_core::Vector,
    ) -> Option<iced_core::overlay::Element<'b, Message, iced_core::Theme, crate::ui::app::Renderer>> {
        self.child.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
    }
}

/// Wraps the pane grid: remembers where it was drawn (so the app can tell
/// which title bar the pointer is over) and asks for a grab pointer there.
pub struct GridFrame<'a> {
    app: &'a App,
    child: Element<'a>,
}

pub fn grid_frame<'a>(app: &'a App, child: impl Into<Element<'a>>) -> Element<'a> {
    iced_core::Element::new(GridFrame { app, child: child.into() })
}

impl<'a> iced_core::Widget<Message, iced_core::Theme, crate::ui::app::Renderer> for GridFrame<'a> {
    fn size(&self) -> iced_core::Size<Length> {
        self.child.as_widget().size()
    }

    fn size_hint(&self) -> iced_core::Size<Length> {
        self.child.as_widget().size_hint()
    }

    fn tag(&self) -> iced_core::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced_core::widget::tree::State {
        self.child.as_widget().state()
    }

    fn children(&self) -> Vec<iced_core::widget::Tree> {
        self.child.as_widget().children()
    }

    fn diff(&self, tree: &mut iced_core::widget::Tree) {
        self.child.as_widget().diff(tree);
    }

    fn layout(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        renderer: &crate::ui::app::Renderer,
        limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        self.child.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        renderer: &crate::ui::app::Renderer,
        operation: &mut dyn iced_core::widget::Operation,
    ) {
        self.child.as_widget_mut().operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        event: &iced_core::Event,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        renderer: &crate::ui::app::Renderer,
        clipboard: &mut dyn iced_core::Clipboard,
        shell: &mut iced_core::Shell<'_, Message>,
        viewport: &iced_core::Rectangle,
    ) {
        self.app.grid_bounds.set(layout.bounds());
        if let iced_core::Event::Mouse(iced_core::mouse::Event::CursorMoved { .. }) = event {
            // The highlight follows the pointer without a message round trip.
            shell.request_redraw();
        }
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        viewport: &iced_core::Rectangle,
        renderer: &crate::ui::app::Renderer,
    ) -> iced_core::mouse::Interaction {
        let inner = self.child.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer);
        if inner != iced_core::mouse::Interaction::None {
            return inner;
        }
        if let Some(p) = cursor.position() {
            if self.app.title_strips().iter().any(|(_, r)| r.contains(p)) {
                return iced_core::mouse::Interaction::Grab;
            }
        }
        inner
    }

    fn draw(
        &self,
        tree: &iced_core::widget::Tree,
        renderer: &mut crate::ui::app::Renderer,
        theme: &iced_core::Theme,
        style: &iced_core::renderer::Style,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        viewport: &iced_core::Rectangle,
    ) {
        self.app.grid_bounds.set(layout.bounds());
        self.child.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced_core::widget::Tree,
        layout: iced_core::Layout<'b>,
        renderer: &crate::ui::app::Renderer,
        viewport: &iced_core::Rectangle,
        translation: iced_core::Vector,
    ) -> Option<iced_core::overlay::Element<'b, Message, iced_core::Theme, crate::ui::app::Renderer>> {
        self.child.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
    }
}

/// Window-style glyphs drawn with quads, so no font has to carry them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// A square outline: maximize.
    Maximize,
    /// Two overlapping squares: restore.
    Restore,
}

struct IconWidget {
    icon: Icon,
    color: Color,
}

const ICON_SIZE: f32 = 12.0;

impl iced_core::Widget<Message, iced_core::Theme, crate::ui::app::Renderer> for IconWidget {
    fn size(&self) -> iced_core::Size<Length> {
        iced_core::Size::new(Length::Fixed(ICON_SIZE), Length::Fixed(ICON_SIZE))
    }

    fn layout(
        &mut self,
        _tree: &mut iced_core::widget::Tree,
        _renderer: &crate::ui::app::Renderer,
        _limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        iced_core::layout::Node::new(iced_core::Size::new(ICON_SIZE, ICON_SIZE))
    }

    fn draw(
        &self,
        _tree: &iced_core::widget::Tree,
        renderer: &mut crate::ui::app::Renderer,
        _theme: &iced_core::Theme,
        _style: &iced_core::renderer::Style,
        layout: iced_core::Layout<'_>,
        _cursor: iced_core::mouse::Cursor,
        _viewport: &iced_core::Rectangle,
    ) {
        use iced_widget::canvas::{Frame, Path, Stroke};
        let b = layout.bounds();
        let (x, y) = (b.x, b.y);
        let stroke = Stroke::default().with_color(self.color).with_width(1.5);
        let mut frame = Frame::with_bounds(renderer, b);
        let pt = |px: f32, py: f32| iced_core::Point::new(x + px, y + py);
        let path = Path::new(|p| {
            // Diagonal from bottom-left to top-right, macOS style.
            p.move_to(pt(2.5, 9.5));
            p.line_to(pt(9.5, 2.5));
            match self.icon {
                Icon::Maximize => {
                    // Heads at the corners, pointing outward.
                    p.move_to(pt(5.5, 2.5));
                    p.line_to(pt(9.5, 2.5));
                    p.line_to(pt(9.5, 6.5));
                    p.move_to(pt(6.5, 9.5));
                    p.line_to(pt(2.5, 9.5));
                    p.line_to(pt(2.5, 5.5));
                }
                Icon::Restore => {
                    // Heads near the center, pointing inward.
                    p.move_to(pt(9.5, 5.5));
                    p.line_to(pt(6.5, 5.5));
                    p.line_to(pt(6.5, 2.5));
                    p.move_to(pt(2.5, 6.5));
                    p.line_to(pt(5.5, 6.5));
                    p.line_to(pt(5.5, 9.5));
                }
            }
        });
        frame.stroke(&path, stroke);
        iced_widget::graphics::geometry::Renderer::draw_geometry(renderer, frame.into_geometry());
    }
}

/// A small square button with a window glyph.
pub fn icon_button<'a>(icon: Icon, color: Color, on_press: Message) -> Element<'a> {
    let glyph: Element<'a> = iced_core::Element::new(IconWidget { icon, color });
    button_widget::Button::new(glyph)
        .padding(4)
        .on_press(on_press)
        .style(|theme: &iced_core::Theme, status| {
            let p = theme.extended_palette();
            button_widget::Style {
                background: match status {
                    button_widget::Status::Hovered => Some(Background::Color(p.background.strong.color)),
                    button_widget::Status::Pressed => Some(Background::Color(p.background.strongest.color)),
                    _ => None,
                },
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
