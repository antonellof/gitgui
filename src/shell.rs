//! Drives iced without a window: terminal events become iced events, the UI
//! is built from `App::view`, updated, drawn with the tiny-skia renderer and
//! copied into the RGBA framebuffer that `term::kitty` ships.

use std::time::Instant;

use iced_core::keyboard::{self, key};
use iced_core::{clipboard, mouse, theme, window, Event, Font, Pixels, Point, Size};
use iced_runtime::user_interface::{self, UserInterface};

use crate::render::frame::Framebuffer;
use crate::term::input::{Event as TermEvent, Key, Mods, MouseButton};
use crate::ui::app::{App, Message};

pub type Renderer = iced_renderer::Renderer;

/// Terminal clipboard: writes go out as OSC 52 (collected here, encoded by
/// the runtime), reads return the last bracketed paste so Ctrl+V works.
#[derive(Default)]
pub struct Clipboard {
    pub copied: Vec<String>,
    pub paste: Option<String>,
}

impl clipboard::Clipboard for Clipboard {
    fn read(&self, _kind: clipboard::Kind) -> Option<String> {
        self.paste.clone()
    }

    fn write(&mut self, _kind: clipboard::Kind, contents: String) {
        self.copied.push(contents);
    }
}

pub struct Shell {
    renderer: Renderer,
    cache: Option<user_interface::Cache>,
    /// Logical size in points.
    size: Size,
    ppp: f32,
    cursor: mouse::Cursor,
    events: Vec<Event>,
    modifiers: keyboard::Modifiers,
    pub clipboard: Clipboard,
    pub theme: iced_core::Theme,
    /// Clip mask reused across frames (one allocation per size).
    mask: Option<tiny_skia::Mask>,
}

/// What a frame asked the runtime for.
pub struct FrameOut {
    pub redraw: window::RedrawRequest,
    /// Text to put on the terminal clipboard.
    pub copy: Vec<String>,
    /// What the pointer hovers, for the terminal's pointer shape.
    pub interaction: mouse::Interaction,
}

impl Shell {
    pub fn new(font_size: f32, ppp: f32, width_px: u32, height_px: u32, theme: iced_core::Theme) -> Self {
        let renderer = Renderer::new(Font::with_name("Fira Sans"), Pixels(font_size));
        let mut s = Shell {
            renderer,
            cache: Some(user_interface::Cache::new()),
            size: Size::ZERO,
            ppp,
            cursor: mouse::Cursor::Unavailable,
            events: Vec::new(),
            modifiers: keyboard::Modifiers::empty(),
            clipboard: Clipboard::default(),
            theme,
            mask: None,
        };
        s.resize(width_px, height_px, ppp);
        s
    }

    pub fn resize(&mut self, width_px: u32, height_px: u32, ppp: f32) {
        self.ppp = ppp;
        self.size = Size::new(width_px as f32 / ppp, height_px as f32 / ppp);
    }

    pub fn modifiers(&self) -> keyboard::Modifiers {
        self.modifiers
    }

    /// Track the modifier state and tell the widgets when it changes:
    /// `text_input` keeps the modifiers it last saw in `ModifiersChanged`,
    /// not the ones on a key press.
    fn set_modifiers(&mut self, m: keyboard::Modifiers) {
        if m != self.modifiers {
            self.modifiers = m;
            self.events.push(Event::Keyboard(keyboard::Event::ModifiersChanged(m)));
        }
    }

    /// Current pointer position in points, if known.
    pub fn cursor(&self) -> Option<Point> {
        self.cursor.position()
    }

    fn point(&self, x: i32, y: i32) -> Point {
        Point::new(x as f32 / self.ppp, y as f32 / self.ppp)
    }

    fn move_to(&mut self, x: i32, y: i32) {
        let p = self.point(x, y);
        if self.cursor.position() != Some(p) {
            self.cursor = mouse::Cursor::Available(p);
            self.events
                .push(Event::Mouse(mouse::Event::CursorMoved { position: p }));
        }
    }

    /// Queue a terminal event for the next frame.
    pub fn push(&mut self, ev: &TermEvent) {
        match ev {
            TermEvent::MouseMove { x, y, mods } => {
                self.modifiers = modifiers(*mods);
                self.move_to(*x, *y);
            }
            TermEvent::MouseButton {
                button,
                pressed,
                x,
                y,
                mods,
            } => {
                self.set_modifiers(modifiers(*mods));
                self.move_to(*x, *y);
                let b = match button {
                    MouseButton::Left => mouse::Button::Left,
                    MouseButton::Middle => mouse::Button::Middle,
                    MouseButton::Right => mouse::Button::Right,
                };
                self.events.push(Event::Mouse(if *pressed {
                    mouse::Event::ButtonPressed(b)
                } else {
                    mouse::Event::ButtonReleased(b)
                }));
            }
            TermEvent::Wheel { dx, dy, x, y, mods } => {
                self.set_modifiers(modifiers(*mods));
                self.move_to(*x, *y);
                self.events.push(Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Lines {
                        x: *dx as f32,
                        y: *dy as f32,
                    },
                }));
            }
            TermEvent::Key {
                key,
                mods,
                text,
                pressed,
                repeat,
            } => {
                self.set_modifiers(modifiers(*mods));
                let Some(k) = iced_key(*key, text.as_deref()) else { return };
                let text = text
                    .as_deref()
                    .filter(|t| !mods.ctrl && !mods.alt && !t.chars().any(|c| c.is_control()))
                    .map(iced_core::SmolStr::new);
                let common = (k.clone(), key::Physical::Unidentified(key::NativeCode::Unidentified));
                self.events.push(Event::Keyboard(if *pressed {
                    keyboard::Event::KeyPressed {
                        key: common.0.clone(),
                        modified_key: common.0,
                        physical_key: common.1,
                        location: keyboard::Location::Standard,
                        modifiers: self.modifiers,
                        text,
                        repeat: *repeat,
                    }
                } else {
                    keyboard::Event::KeyReleased {
                        key: common.0.clone(),
                        modified_key: common.0,
                        physical_key: common.1,
                        location: keyboard::Location::Standard,
                        modifiers: self.modifiers,
                    }
                }));
            }
            TermEvent::Paste(s) => {
                // Bracketed paste: hand the text to the focused editor through
                // its own paste key, which reads our clipboard. iced binds
                // paste to `Modifiers::COMMAND`: Cmd on macOS, Ctrl elsewhere.
                self.clipboard.paste = Some(s.clone());
                let k = keyboard::Key::Character(iced_core::SmolStr::new("v"));
                let phys = key::Physical::Unidentified(key::NativeCode::Unidentified);
                let mods = keyboard::Modifiers::COMMAND;
                // text_input reads the modifiers it saw in ModifiersChanged,
                // text_editor the ones on the key press: send both.
                self.events.push(Event::Keyboard(keyboard::Event::ModifiersChanged(mods)));
                self.events.push(Event::Keyboard(keyboard::Event::KeyPressed {
                    key: k.clone(),
                    modified_key: k.clone(),
                    physical_key: phys,
                    location: keyboard::Location::Standard,
                    modifiers: mods,
                    text: None,
                    repeat: false,
                }));
                self.events.push(Event::Keyboard(keyboard::Event::KeyReleased {
                    key: k.clone(),
                    modified_key: k,
                    physical_key: phys,
                    location: keyboard::Location::Standard,
                    modifiers: mods,
                }));
                self.events.push(Event::Keyboard(keyboard::Event::ModifiersChanged(self.modifiers)));
            }
            TermEvent::Focus(f) => {
                self.events.push(Event::Window(if *f {
                    window::Event::Focused
                } else {
                    window::Event::Unfocused
                }));
            }
            TermEvent::Unknown(_) => {}
        }
    }

    /// Build, update and draw one frame into `fb`.
    pub fn frame(&mut self, app: &mut App, fb: &mut Framebuffer) -> FrameOut {
        let phys = Size::new(fb.width(), fb.height());
        if (self.size.width * self.ppp).round() as u32 != phys.width
            || (self.size.height * self.ppp).round() as u32 != phys.height
        {
            self.resize(phys.width, phys.height, self.ppp);
        }
        app.window = self.size;
        if let Some(p) = self.cursor.position() {
            app.cursor = p;
        }
        let mut events = std::mem::take(&mut self.events);
        events.push(Event::Window(window::Event::RedrawRequested(Instant::now())));
        let mut redraw = window::RedrawRequest::Wait;
        let mut interaction = mouse::Interaction::None;
        let mut messages: Vec<Message> = Vec::new();
        let mut ops: Vec<Box<dyn iced_core::widget::Operation>> = std::mem::take(&mut app.ops);

        // Build, deliver events, apply messages, rebuild; the UI that saw no
        // new messages is the one drawn. Idle frames build exactly once.
        let mut rounds = 0;
        loop {
            rounds += 1;
            let cache = self.cache.take().unwrap_or_default();
            let mut ui = UserInterface::build(app.view(), self.size, cache, &mut self.renderer);
            for op in ops.drain(..) {
                let mut op = op;
                ui.operate(&self.renderer, op.as_mut());
            }
            let (state, statuses) = ui.update(
                &events,
                self.cursor,
                &mut self.renderer,
                &mut self.clipboard,
                &mut messages,
            );
            if let user_interface::State::Updated {
                redraw_request,
                mouse_interaction,
                ..
            } = state
            {
                redraw = merge_redraw(redraw, redraw_request);
                interaction = mouse_interaction;
            }
            for (ev, status) in events.iter().zip(statuses) {
                if status == iced_core::event::Status::Ignored {
                    if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = ev {
                        messages.push(Message::Key(key.clone(), *modifiers));
                    }
                }
            }
            self.clipboard.paste = None;
            if messages.is_empty() || rounds >= 4 {
                let base = theme::Base::base(&self.theme);
                ui.draw(
                    &mut self.renderer,
                    &self.theme,
                    &iced_core::renderer::Style {
                        text_color: base.text_color,
                    },
                    self.cursor,
                );
                self.cache = Some(ui.into_cache());
                self.present(fb, base.background_color);
                break;
            }
            self.cache = Some(ui.into_cache());
            for m in messages.drain(..) {
                app.update(m);
            }
            ops.append(&mut app.ops);
            events.clear();
            events.push(Event::Window(window::Event::RedrawRequested(Instant::now())));
        }
        if app.toasts_active() {
            redraw = merge_redraw(
                redraw,
                window::RedrawRequest::At(Instant::now() + std::time::Duration::from_millis(500)),
            );
        }
        FrameOut {
            redraw,
            copy: std::mem::take(&mut self.clipboard.copied),
            interaction,
        }
    }

    /// Rasterize the drawn layers straight into the framebuffer.
    fn present(&mut self, fb: &mut Framebuffer, background: iced_core::Color) {
        let (w, h) = (fb.width(), fb.height());
        if self.mask.as_ref().map(|m| (m.width(), m.height())) != Some((w, h)) {
            self.mask = tiny_skia::Mask::new(w, h);
        }
        let Some(mask) = self.mask.as_mut() else { return };
        let viewport = iced_widget::graphics::Viewport::with_physical_size(Size::new(w, h), self.ppp);
        let Some(mut pixmap) = tiny_skia::PixmapMut::from_bytes(fb.pixels_mut(), w, h) else { return };
        self.renderer.draw(
            &mut pixmap,
            mask,
            &viewport,
            &[iced_core::Rectangle::with_size(Size::new(w as f32, h as f32))],
            background,
        );
        // tiny-skia keeps BGRA in memory; the kitty encoder wants RGBA. The
        // frame is opaque so premultiplication does not matter.
        for px in fb.pixels_mut().as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
}

fn merge_redraw(a: window::RedrawRequest, b: window::RedrawRequest) -> window::RedrawRequest {
    use window::RedrawRequest::{At, NextFrame, Wait};
    match (a, b) {
        (NextFrame, _) | (_, NextFrame) => NextFrame,
        (At(x), At(y)) => At(x.min(y)),
        (At(x), Wait) | (Wait, At(x)) => At(x),
        (Wait, Wait) => Wait,
    }
}

/// Terminal modifiers as iced modifiers. iced binds copy, cut, paste, select
/// all and friends to `Modifiers::COMMAND`, which is Cmd on macOS; a terminal
/// only ever delivers Ctrl (Cmd belongs to the terminal), so Ctrl also sets
/// the command bit there. `control()` stays true for our own bindings.
fn modifiers(m: Mods) -> keyboard::Modifiers {
    let mut out = keyboard::Modifiers::empty();
    if m.shift {
        out |= keyboard::Modifiers::SHIFT;
    }
    if m.ctrl {
        out |= keyboard::Modifiers::CTRL | keyboard::Modifiers::COMMAND;
    }
    if m.alt {
        out |= keyboard::Modifiers::ALT;
    }
    if m.sup {
        out |= keyboard::Modifiers::LOGO;
    }
    out
}

fn iced_key(k: Key, text: Option<&str>) -> Option<keyboard::Key> {
    use keyboard::key::Named as N;
    Some(match k {
        Key::Enter => keyboard::Key::Named(N::Enter),
        Key::Tab => keyboard::Key::Named(N::Tab),
        Key::Escape => keyboard::Key::Named(N::Escape),
        Key::Backspace => keyboard::Key::Named(N::Backspace),
        Key::Insert => keyboard::Key::Named(N::Insert),
        Key::Delete => keyboard::Key::Named(N::Delete),
        Key::Left => keyboard::Key::Named(N::ArrowLeft),
        Key::Right => keyboard::Key::Named(N::ArrowRight),
        Key::Up => keyboard::Key::Named(N::ArrowUp),
        Key::Down => keyboard::Key::Named(N::ArrowDown),
        Key::PageUp => keyboard::Key::Named(N::PageUp),
        Key::PageDown => keyboard::Key::Named(N::PageDown),
        Key::Home => keyboard::Key::Named(N::Home),
        Key::End => keyboard::Key::Named(N::End),
        Key::F(n) => keyboard::Key::Named(match n {
            1 => N::F1,
            2 => N::F2,
            3 => N::F3,
            4 => N::F4,
            5 => N::F5,
            6 => N::F6,
            7 => N::F7,
            8 => N::F8,
            9 => N::F9,
            10 => N::F10,
            11 => N::F11,
            12 => N::F12,
            _ => return None,
        }),
        Key::Char(' ') => keyboard::Key::Named(N::Space),
        Key::Char(c) => {
            // The text carries the shifted character when there is one.
            let s = match text {
                Some(t) if t.chars().count() == 1 && !t.chars().any(|c| c.is_control()) => t.to_owned(),
                _ => c.to_string(),
            };
            keyboard::Key::Character(iced_core::SmolStr::new(s))
        }
        Key::Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_iced_keys() {
        assert_eq!(iced_key(Key::Enter, None), Some(keyboard::Key::Named(key::Named::Enter)));
        assert_eq!(
            iced_key(Key::Char('a'), Some("A")),
            Some(keyboard::Key::Character(iced_core::SmolStr::new("A")))
        );
        assert_eq!(iced_key(Key::Char(' '), Some(" ")), Some(keyboard::Key::Named(key::Named::Space)));
        assert_eq!(iced_key(Key::Other(57441), None), None);
    }

    #[test]
    fn mouse_events_carry_points() {
        let theme = crate::ui::theme::Theme::dark().iced();
        let mut s = Shell::new(13.0, 2.0, 200, 100, theme);
        s.push(&TermEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
            x: 40,
            y: 20,
            mods: Mods::NONE,
        });
        assert_eq!(s.cursor(), Some(Point::new(20.0, 10.0)));
        assert!(matches!(
            s.events.as_slice(),
            [
                Event::Mouse(mouse::Event::CursorMoved { .. }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            ]
        ));
        s.push(&TermEvent::Wheel {
            dx: 0,
            dy: -1,
            x: 40,
            y: 20,
            mods: Mods::NONE,
        });
        assert!(matches!(
            s.events.last(),
            Some(Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { y, .. }
            })) if *y == -1.0
        ));
    }
}
