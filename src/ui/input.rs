//! Terminal events to egui events (docs/SPEC.md 2.3).

use crate::term::input::{Event, Key, Mods, MouseButton};

/// Scroll distance per wheel notch in device pixels.
const WHEEL_PIXELS_PER_NOTCH: f32 = 40.0;

pub struct Mapper {
    ppp: f32,
    wheel: egui::Vec2,
    wheel_mods: egui::Modifiers,
    pointer: egui::Pos2,
    down: [bool; 3],
}

fn modifiers(m: Mods) -> egui::Modifiers {
    egui::Modifiers {
        alt: m.alt,
        ctrl: m.ctrl,
        shift: m.shift,
        mac_cmd: false,
        // egui uses `command` for its shortcuts (select all, undo, ...).
        // Terminals never deliver Cmd, so Ctrl plays that role.
        command: m.ctrl,
    }
}

fn egui_key(k: Key) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match k {
        Key::Enter => K::Enter,
        Key::Tab => K::Tab,
        Key::Escape => K::Escape,
        Key::Backspace => K::Backspace,
        Key::Insert => K::Insert,
        Key::Delete => K::Delete,
        Key::Left => K::ArrowLeft,
        Key::Right => K::ArrowRight,
        Key::Up => K::ArrowUp,
        Key::Down => K::ArrowDown,
        Key::PageUp => K::PageUp,
        Key::PageDown => K::PageDown,
        Key::Home => K::Home,
        Key::End => K::End,
        Key::F(n) => K::from_name(&format!("F{n}"))?,
        Key::Char(c) => {
            let mut s = String::new();
            s.push(c.to_ascii_uppercase());
            K::from_name(&s)?
        }
        Key::Other(_) => return None,
    })
}

fn egui_button(b: MouseButton) -> egui::PointerButton {
    match b {
        MouseButton::Left => egui::PointerButton::Primary,
        MouseButton::Middle => egui::PointerButton::Middle,
        MouseButton::Right => egui::PointerButton::Secondary,
    }
}

impl Mapper {
    pub fn new(ppp: f32) -> Self {
        Self {
            ppp,
            wheel: egui::Vec2::ZERO,
            wheel_mods: egui::Modifiers::NONE,
            pointer: egui::Pos2::ZERO,
            down: [false; 3],
        }
    }

    fn pos(&self, x: i32, y: i32) -> egui::Pos2 {
        egui::pos2(x as f32 / self.ppp, y as f32 / self.ppp)
    }

    /// Translate one terminal event. Wheel notches are accumulated; call
    /// [`flush`] once per batch to emit them as one scroll event.
    pub fn map(&mut self, ev: &Event, out: &mut Vec<egui::Event>) {
        match ev {
            Event::Key { key, mods, text, pressed, repeat } => {
                let m = modifiers(*mods);
                if let Some(k) = egui_key(*key) {
                    out.push(egui::Event::Key {
                        key: k,
                        physical_key: None,
                        pressed: *pressed,
                        repeat: *repeat,
                        modifiers: m,
                    });
                }
                if *pressed && !mods.ctrl && !mods.alt && !mods.sup {
                    if let Some(t) = text {
                        let is_control = matches!(key, Key::Enter | Key::Tab | Key::Escape | Key::Backspace)
                            || t.chars().any(|c| c.is_control());
                        if !is_control && !t.is_empty() {
                            out.push(egui::Event::Text(t.clone()));
                        }
                    }
                }
            }
            Event::MouseButton { button, pressed, x, y, mods } => {
                let pos = self.pos(*x, *y);
                if pos != self.pointer {
                    self.pointer = pos;
                    out.push(egui::Event::PointerMoved(pos));
                }
                let idx = match button {
                    MouseButton::Left => 0,
                    MouseButton::Middle => 1,
                    MouseButton::Right => 2,
                };
                self.down[idx] = *pressed;
                out.push(egui::Event::PointerButton {
                    pos,
                    button: egui_button(*button),
                    pressed: *pressed,
                    modifiers: modifiers(*mods),
                });
            }
            Event::MouseMove { x, y, .. } => {
                let pos = self.pos(*x, *y);
                if pos != self.pointer {
                    self.pointer = pos;
                    out.push(egui::Event::PointerMoved(pos));
                }
            }
            Event::Wheel { dx, dy, x, y, mods } => {
                let pos = self.pos(*x, *y);
                if pos != self.pointer {
                    self.pointer = pos;
                    out.push(egui::Event::PointerMoved(pos));
                }
                let step = WHEEL_PIXELS_PER_NOTCH / self.ppp;
                self.wheel += egui::vec2(*dx as f32 * step, *dy as f32 * step);
                self.wheel_mods = modifiers(*mods);
            }
            Event::Paste(text) => out.push(egui::Event::Paste(text.clone())),
            Event::Focus(focused) => {
                out.push(egui::Event::WindowFocused(*focused));
                if !focused {
                    self.release_all(out);
                    out.push(egui::Event::PointerGone);
                }
            }
            Event::Unknown(_) => {}
        }
    }

    /// Emit the accumulated wheel delta, if any.
    pub fn flush(&mut self, out: &mut Vec<egui::Event>) {
        if self.wheel != egui::Vec2::ZERO {
            out.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: self.wheel,
                modifiers: self.wheel_mods,
                phase: egui::TouchPhase::Move,
            });
            self.wheel = egui::Vec2::ZERO;
        }
    }

    /// Release every pressed button (focus lost).
    pub fn release_all(&mut self, out: &mut Vec<egui::Event>) {
        let buttons = [egui::PointerButton::Primary, egui::PointerButton::Middle, egui::PointerButton::Secondary];
        for (i, b) in buttons.iter().enumerate() {
            if self.down[i] {
                self.down[i] = false;
                out.push(egui::Event::PointerButton {
                    pos: self.pointer,
                    button: *b,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, mods: Mods, text: Option<&str>) -> Event {
        Event::Key { key: k, mods, text: text.map(|s| s.to_owned()), pressed: true, repeat: false }
    }

    #[test]
    fn letter_gives_key_and_text() {
        let mut m = Mapper::new(1.0);
        let mut out = Vec::new();
        m.map(&key(Key::Char('j'), Mods::NONE, Some("j")), &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], egui::Event::Key { key: egui::Key::J, pressed: true, .. }));
        assert_eq!(out[1], egui::Event::Text("j".into()));
    }

    #[test]
    fn control_keys_give_no_text() {
        let mut m = Mapper::new(1.0);
        let mut out = Vec::new();
        m.map(&key(Key::Enter, Mods::NONE, None), &mut out);
        m.map(&key(Key::Char('c'), Mods::CTRL, None), &mut out);
        m.map(&key(Key::Char('a'), Mods::CTRL, Some("a")), &mut out);
        assert!(out.iter().all(|e| !matches!(e, egui::Event::Text(_))));
        assert!(matches!(out[1], egui::Event::Key { key: egui::Key::C, modifiers, .. } if modifiers.ctrl && modifiers.command));
    }

    #[test]
    fn punctuation_and_function_keys_map() {
        let mut m = Mapper::new(1.0);
        let mut out = Vec::new();
        m.map(&key(Key::Char('/'), Mods::NONE, Some("/")), &mut out);
        m.map(&key(Key::F(5), Mods::NONE, None), &mut out);
        m.map(&key(Key::Other(57441), Mods::SHIFT, None), &mut out);
        assert!(matches!(out[0], egui::Event::Key { key: egui::Key::Slash, .. }));
        assert!(matches!(out[2], egui::Event::Key { key: egui::Key::F5, .. }));
        assert_eq!(out.len(), 3, "modifier key alone produces nothing");
    }

    #[test]
    fn mouse_positions_scale_by_ppp() {
        let mut m = Mapper::new(2.0);
        let mut out = Vec::new();
        m.map(&Event::MouseButton { button: MouseButton::Left, pressed: true, x: 100, y: 50, mods: Mods::NONE }, &mut out);
        assert_eq!(out[0], egui::Event::PointerMoved(egui::pos2(50.0, 25.0)));
        assert!(matches!(out[1], egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } if pos == egui::pos2(50.0, 25.0)));
        out.clear();
        m.map(&Event::MouseMove { x: 100, y: 50, mods: Mods::NONE }, &mut out);
        assert!(out.is_empty(), "no move event for the same position");
        m.map(&Event::MouseMove { x: 102, y: 50, mods: Mods::NONE }, &mut out);
        assert_eq!(out, vec![egui::Event::PointerMoved(egui::pos2(51.0, 25.0))]);
    }

    #[test]
    fn wheel_is_coalesced() {
        let mut m = Mapper::new(2.0);
        let mut out = Vec::new();
        for _ in 0..3 {
            m.map(&Event::Wheel { dx: 0, dy: -1, x: 10, y: 10, mods: Mods::NONE }, &mut out);
        }
        m.flush(&mut out);
        let wheels: Vec<_> = out.iter().filter(|e| matches!(e, egui::Event::MouseWheel { .. })).collect();
        assert_eq!(wheels.len(), 1);
        assert!(matches!(wheels[0], egui::Event::MouseWheel { delta, .. } if *delta == egui::vec2(0.0, -60.0)));
        out.clear();
        m.flush(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn focus_lost_releases_buttons() {
        let mut m = Mapper::new(1.0);
        let mut out = Vec::new();
        m.map(&Event::MouseButton { button: MouseButton::Left, pressed: true, x: 1, y: 1, mods: Mods::NONE }, &mut out);
        out.clear();
        m.map(&Event::Focus(false), &mut out);
        assert_eq!(out[0], egui::Event::WindowFocused(false));
        assert!(matches!(out[1], egui::Event::PointerButton { pressed: false, .. }));
        assert_eq!(out[2], egui::Event::PointerGone);
    }

    #[test]
    fn paste_maps() {
        let mut m = Mapper::new(1.0);
        let mut out = Vec::new();
        m.map(&Event::Paste("x".into()), &mut out);
        assert_eq!(out, vec![egui::Event::Paste("x".into())]);
    }
}
