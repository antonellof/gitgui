//! Terminal input decoding: bytes from stdin to [`Event`]s.
//!
//! Covers the kitty keyboard protocol, the legacy CSI key forms, plain
//! UTF-8 text with the Ctrl and Alt fallbacks, SGR mouse in pixel or cell
//! coordinates, focus events and bracketed paste. See docs/PROTOCOLS.md
//! section 3. The parser is pure: feed bytes, get events; incomplete
//! sequences stay buffered until more bytes or a [`Parser::flush`].

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub sup: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: false,
    };
    #[cfg(test)]
    pub const SHIFT: Mods = Mods {
        shift: true,
        alt: false,
        ctrl: false,
        sup: false,
    };
    pub const ALT: Mods = Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: false,
    };
    pub const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    };

    /// From the kitty / xterm modifier parameter (`1 + bits`).
    fn from_param(p: u32) -> Mods {
        let bits = p.saturating_sub(1);
        Mods {
            shift: bits & 1 != 0,
            alt: bits & 2 != 0,
            ctrl: bits & 4 != 0,
            sup: bits & 8 != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Escape,
    Backspace,
    Insert,
    Delete,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    F(u8),
    /// A key we do not map, with its kitty codepoint (modifier keys etc).
    Other(u32),
}

impl Key {
    fn from_codepoint(cp: u32) -> Key {
        match cp {
            13 => Key::Enter,
            9 => Key::Tab,
            27 => Key::Escape,
            127 | 8 => Key::Backspace,
            57348 => Key::Insert,
            57349 => Key::Delete,
            57350 => Key::Left,
            57351 => Key::Right,
            57352 => Key::Up,
            57353 => Key::Down,
            57354 => Key::PageUp,
            57355 => Key::PageDown,
            57356 => Key::Home,
            57357 => Key::End,
            57364..=57375 => Key::F((cp - 57364 + 1) as u8),
            // Keypad keys map to their plain equivalents.
            57399..=57408 => Key::Char(char::from_u32(b'0' as u32 + cp - 57399).unwrap_or('0')),
            57414 => Key::Enter,
            57417 => Key::Left,
            57418 => Key::Right,
            57419 => Key::Up,
            57420 => Key::Down,
            57421 => Key::PageUp,
            57422 => Key::PageDown,
            57423 => Key::Home,
            57424 => Key::End,
            57425 => Key::Insert,
            57426 => Key::Delete,
            _ => match char::from_u32(cp) {
                Some(c) if cp >= 32 && !(0xE000..=0xF8FF).contains(&cp) => Key::Char(c),
                _ => Key::Other(cp),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key {
        key: Key,
        mods: Mods,
        /// Text produced by this key press, if any.
        text: Option<String>,
        pressed: bool,
        repeat: bool,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
        x: i32,
        y: i32,
        mods: Mods,
    },
    MouseMove {
        x: i32,
        y: i32,
        mods: Mods,
    },
    /// One wheel notch. `dy` is +1 for up, -1 for down; `dx` +1 for left, -1 for right.
    Wheel {
        dx: i32,
        dy: i32,
        x: i32,
        y: i32,
        mods: Mods,
    },
    Paste(String),
    Focus(bool),
    /// A complete sequence we did not understand.
    Unknown(Vec<u8>),
}

pub struct Parser {
    buf: Vec<u8>,
    /// SGR mouse reports pixels (`?1016` accepted); otherwise cells.
    pub pixel_mouse: bool,
    pub cell_w: u32,
    pub cell_h: u32,
}

enum Step {
    /// Consumed n bytes, produced an event (or none for ignored sequences).
    Done(usize, Option<Event>),
    /// Need more bytes.
    Incomplete,
}

impl Parser {
    pub fn new(pixel_mouse: bool, cell_w: u32, cell_h: u32) -> Self {
        Self {
            buf: Vec::with_capacity(256),
            pixel_mouse,
            cell_w: cell_w.max(1),
            cell_h: cell_h.max(1),
        }
    }

    /// True while an incomplete sequence is buffered (call [`flush`] after
    /// the escape timeout).
    pub fn has_pending(&self) -> bool {
        !self.buf.is_empty()
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        self.drain(&mut out, false);
        out
    }

    /// Resolve buffered bytes that will not be completed: a lone ESC becomes
    /// the Escape key, anything else is emitted as [`Event::Unknown`].
    pub fn flush(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        self.drain(&mut out, true);
        out
    }

    fn drain(&mut self, out: &mut Vec<Event>, force: bool) {
        loop {
            if self.buf.is_empty() {
                return;
            }
            match self.step() {
                Step::Done(n, ev) => {
                    self.buf.drain(..n);
                    if let Some(ev) = ev {
                        out.push(ev);
                    }
                }
                Step::Incomplete => {
                    if !force {
                        return;
                    }
                    if self.buf == [0x1b] {
                        self.buf.clear();
                        out.push(key_event(Key::Escape, Mods::NONE, None));
                    } else if self.buf[0] == 0x1b
                        && self.buf.len() >= 2
                        && self.buf[1] != b'['
                        && self.buf[1] != b'O'
                    {
                        // ESC + byte fallback: Alt+byte.
                        let b = self.buf[1];
                        self.buf.drain(..2);
                        out.push(alt_byte(b));
                    } else {
                        let rest = std::mem::take(&mut self.buf);
                        out.push(Event::Unknown(rest));
                    }
                    return;
                }
            }
        }
    }

    fn step(&self) -> Step {
        let b = &self.buf;
        if b[0] != 0x1b {
            return self.step_plain();
        }
        if b.len() == 1 {
            return Step::Incomplete;
        }
        match b[1] {
            b'[' => self.step_csi(),
            b'O' => {
                // SS3 legacy: ESC O A..D, H, F, P..S
                let Some(&f) = b.get(2) else {
                    return Step::Incomplete;
                };
                let key = match f {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'C' => Some(Key::Right),
                    b'D' => Some(Key::Left),
                    b'H' => Some(Key::Home),
                    b'F' => Some(Key::End),
                    b'P' => Some(Key::F(1)),
                    b'Q' => Some(Key::F(2)),
                    b'R' => Some(Key::F(3)),
                    b'S' => Some(Key::F(4)),
                    _ => None,
                };
                match key {
                    Some(k) => Step::Done(3, Some(key_event(k, Mods::NONE, None))),
                    None => Step::Done(3, Some(Event::Unknown(b[..3].to_vec()))),
                }
            }
            b'_' | b']' | b'P' | b'^' => {
                // APC / OSC / DCS / PM: skip until ST (or BEL for OSC).
                let body = &b[2..];
                let st = body.windows(2).position(|w| w == b"\x1b\\").map(|p| p + 2);
                let bel = if b[1] == b']' {
                    body.iter().position(|&c| c == 0x07).map(|p| p + 1)
                } else {
                    None
                };
                match (st, bel) {
                    (Some(s), Some(l)) => Step::Done(2 + s.min(l), None),
                    (Some(s), None) => Step::Done(2 + s, None),
                    (None, Some(l)) => Step::Done(2 + l, None),
                    (None, None) => Step::Incomplete,
                }
            }
            0x1b => {
                // ESC ESC: the first one is a lone Escape.
                Step::Done(1, Some(key_event(Key::Escape, Mods::NONE, None)))
            }
            other => {
                // Alt + byte (legacy). If it looks like the start of UTF-8,
                // wait for the whole char.
                if other >= 0x80 {
                    let need = utf8_len(other);
                    if b.len() < 1 + need {
                        return Step::Incomplete;
                    }
                    if let Ok(s) = std::str::from_utf8(&b[1..1 + need]) {
                        let c = s.chars().next().unwrap_or('?');
                        return Step::Done(
                            1 + need,
                            Some(key_event(Key::Char(c), Mods::ALT, None)),
                        );
                    }
                    return Step::Done(1 + need, Some(Event::Unknown(b[..1 + need].to_vec())));
                }
                Step::Done(2, Some(alt_byte(other)))
            }
        }
    }

    fn step_plain(&self) -> Step {
        let b = &self.buf;
        let c = b[0];
        if c < 0x20 || c == 0x7f {
            let ev = match c {
                0x0d => key_event(Key::Enter, Mods::NONE, None),
                0x0a => key_event(Key::Enter, Mods::CTRL, None),
                0x09 => key_event(Key::Tab, Mods::NONE, None),
                0x7f | 0x08 => key_event(Key::Backspace, Mods::NONE, None),
                0x00 => key_event(Key::Char(' '), Mods::CTRL, None),
                0x01..=0x1a => key_event(Key::Char((b'a' + c - 1) as char), Mods::CTRL, None),
                0x1c..=0x1f => key_event(Key::Char((b'4' + c - 0x1c) as char), Mods::CTRL, None),
                _ => Event::Unknown(vec![c]),
            };
            return Step::Done(1, Some(ev));
        }
        let need = utf8_len(c);
        if b.len() < need {
            return Step::Incomplete;
        }
        match std::str::from_utf8(&b[..need]) {
            Ok(s) => {
                let ch = s.chars().next().unwrap_or('?');
                Step::Done(
                    need,
                    Some(key_event(Key::Char(ch), Mods::NONE, Some(ch.to_string()))),
                )
            }
            Err(_) => Step::Done(1, Some(Event::Unknown(vec![c]))),
        }
    }

    fn step_csi(&self) -> Step {
        let b = &self.buf;
        // ESC [ params (0x30..0x3F) intermediates (0x20..0x2F) final (0x40..0x7E)
        let body = &b[2..];
        let Some(fin_idx) = body.iter().position(|&c| (0x40..=0x7e).contains(&c)) else {
            // Bracketed paste start without its end yet is also incomplete,
            // and so is any truncated CSI.
            return Step::Incomplete;
        };
        let fin = body[fin_idx];
        let params = &body[..fin_idx];
        let total = 2 + fin_idx + 1;

        // Bracketed paste: CSI 200 ~ ... CSI 201 ~
        if fin == b'~' && params == b"200" {
            let rest = &b[total..];
            let Some(end) = rest.windows(6).position(|w| w == b"\x1b[201~") else {
                return Step::Incomplete;
            };
            let text = String::from_utf8_lossy(&rest[..end]).into_owned();
            return Step::Done(total + end + 6, Some(Event::Paste(text)));
        }

        let ev = match fin {
            b'u' => parse_kitty_key(params),
            b'M' | b'm' if params.first() == Some(&b'<') => {
                self.parse_sgr_mouse(&params[1..], fin == b'M')
            }
            b'I' if params.is_empty() => Some(Event::Focus(true)),
            b'O' if params.is_empty() => Some(Event::Focus(false)),
            b'A' | b'B' | b'C' | b'D' | b'H' | b'F' | b'P' | b'Q' | b'S' | b'Z' => {
                let key = match fin {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    b'P' => Key::F(1),
                    b'Q' => Key::F(2),
                    b'S' => Key::F(4),
                    _ => Key::Tab,
                };
                // CSI 1 ; mod X  or  CSI X.  CSI Z is shift-tab.
                let mut mods = legacy_mods(params);
                if fin == b'Z' {
                    mods.shift = true;
                }
                Some(key_event(key, mods, None))
            }
            b'~' => {
                let mut it = params.split(|&c| c == b';');
                let num: u32 = parse_num(it.next().unwrap_or(b"")).unwrap_or(0);
                let mods = it
                    .next()
                    .and_then(parse_num)
                    .map(Mods::from_param)
                    .unwrap_or(Mods::NONE);
                let key = match num {
                    2 => Some(Key::Insert),
                    3 => Some(Key::Delete),
                    5 => Some(Key::PageUp),
                    6 => Some(Key::PageDown),
                    1 | 7 => Some(Key::Home),
                    4 | 8 => Some(Key::End),
                    11..=15 => Some(Key::F((num - 10) as u8)),
                    17..=21 => Some(Key::F((num - 11) as u8)),
                    23 | 24 => Some(Key::F((num - 12) as u8)),
                    _ => None,
                };
                key.map(|k| key_event(k, mods, None))
            }
            _ => None,
        };
        match ev {
            Some(ev) => Step::Done(total, Some(ev)),
            None => Step::Done(total, Some(Event::Unknown(b[..total].to_vec()))),
        }
    }

    fn parse_sgr_mouse(&self, params: &[u8], is_press_or_motion: bool) -> Option<Event> {
        let mut it = params.split(|&c| c == b';');
        let cb: u32 = parse_num(it.next()?)?;
        let px: i32 = parse_num(it.next()?)? as i32;
        let py: i32 = parse_num(it.next()?)? as i32;
        let (x, y) = if self.pixel_mouse {
            ((px - 1).max(0), (py - 1).max(0))
        } else {
            (
                (px - 1).max(0) * self.cell_w as i32 + self.cell_w as i32 / 2,
                (py - 1).max(0) * self.cell_h as i32 + self.cell_h as i32 / 2,
            )
        };
        let mods = Mods {
            shift: cb & 4 != 0,
            alt: cb & 8 != 0,
            ctrl: cb & 16 != 0,
            sup: false,
        };
        if cb & 64 != 0 {
            let (dx, dy) = match cb & 3 {
                0 => (0, 1),
                1 => (0, -1),
                2 => (1, 0),
                _ => (-1, 0),
            };
            return Some(Event::Wheel { dx, dy, x, y, mods });
        }
        if cb & 32 != 0 {
            return Some(Event::MouseMove { x, y, mods });
        }
        let button = match cb & 3 {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => return Some(Event::MouseMove { x, y, mods }),
        };
        Some(Event::MouseButton {
            button,
            pressed: is_press_or_motion,
            x,
            y,
            mods,
        })
    }
}

fn key_event(key: Key, mods: Mods, text: Option<String>) -> Event {
    Event::Key {
        key,
        mods,
        text,
        pressed: true,
        repeat: false,
    }
}

fn alt_byte(b: u8) -> Event {
    if b < 0x20 || b == 0x7f {
        // Alt + control byte: reuse the control mapping and add alt.
        let mut p = Parser::new(true, 1, 1);
        let mut evs = p.feed(&[b]);
        if let Some(Event::Key { mods, .. }) = evs.first_mut() {
            mods.alt = true;
        }
        return evs.pop().unwrap_or(Event::Unknown(vec![0x1b, b]));
    }
    key_event(Key::Char(b as char), Mods::ALT, None)
}

fn legacy_mods(params: &[u8]) -> Mods {
    // "1;5" -> modifier 5. A bare "5" (no key number) is not standard but accept it.
    let mut it = params.split(|&c| c == b';');
    let first = it.next().unwrap_or(b"");
    match it.next() {
        Some(m) => parse_num(m).map(Mods::from_param).unwrap_or(Mods::NONE),
        None if !first.is_empty() && first != b"1" => {
            parse_num(first).map(Mods::from_param).unwrap_or(Mods::NONE)
        }
        None => Mods::NONE,
    }
}

fn parse_num(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    std::str::from_utf8(s).ok()?.parse().ok()
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

/// `CSI key[:shifted[:base]] ; mods[:event] [; text...] u`
fn parse_kitty_key(params: &[u8]) -> Option<Event> {
    let mut fields = params.split(|&c| c == b';');
    let keyf = fields.next()?;
    let modf = fields.next().unwrap_or(b"1");
    let textf = fields.next();

    let mut keyparts = keyf.split(|&c| c == b':');
    let cp: u32 = parse_num(keyparts.next()?)?;
    let shifted: Option<u32> = keyparts.next().and_then(parse_num);
    let _base: Option<u32> = keyparts.next().and_then(parse_num);

    let mut modparts = modf.split(|&c| c == b':');
    let modp = parse_num(modparts.next().unwrap_or(b"1")).unwrap_or(1);
    let event = modparts.next().and_then(parse_num).unwrap_or(1);
    // Mask lock keys (caps 64, num 128) out of the modifier bits.
    let mods = Mods::from_param(((modp.saturating_sub(1)) & 0x3f) + 1);
    let pressed = event != 3;
    let repeat = event == 2;

    let key = Key::from_codepoint(cp);
    let text = if !pressed {
        None
    } else if let Some(t) = textf {
        let s: String = t
            .split(|&c| c == b':')
            .filter_map(parse_num)
            .filter_map(char::from_u32)
            .collect();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    } else if mods.ctrl || mods.alt || mods.sup {
        None
    } else {
        match key {
            Key::Char(c) => {
                if mods.shift {
                    if let Some(sc) = shifted.and_then(char::from_u32) {
                        Some(sc.to_string())
                    } else if c.is_ascii() {
                        Some(c.to_ascii_uppercase().to_string())
                    } else {
                        Some(c.to_string())
                    }
                } else {
                    Some(c.to_string())
                }
            }
            _ => None,
        }
    };
    Some(Event::Key {
        key,
        mods,
        text,
        pressed,
        repeat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bytes: &[u8]) -> Vec<Event> {
        Parser::new(true, 16, 34).feed(bytes)
    }

    fn key(k: Key, mods: Mods, text: Option<&str>) -> Event {
        Event::Key {
            key: k,
            mods,
            text: text.map(|s| s.to_owned()),
            pressed: true,
            repeat: false,
        }
    }

    #[test]
    fn kitty_plain_letter_has_text() {
        assert_eq!(
            feed(b"\x1b[113u"),
            vec![key(Key::Char('q'), Mods::NONE, Some("q"))]
        );
        assert_eq!(
            feed(b"\x1b[113;1u"),
            vec![key(Key::Char('q'), Mods::NONE, Some("q"))]
        );
        assert_eq!(
            feed(b"\x1b[113;1:1u"),
            vec![key(Key::Char('q'), Mods::NONE, Some("q"))]
        );
    }

    #[test]
    fn kitty_repeat_and_release() {
        assert_eq!(
            feed(b"\x1b[106;1:2u"),
            vec![Event::Key {
                key: Key::Char('j'),
                mods: Mods::NONE,
                text: Some("j".into()),
                pressed: true,
                repeat: true
            }]
        );
        assert_eq!(
            feed(b"\x1b[106;1:3u"),
            vec![Event::Key {
                key: Key::Char('j'),
                mods: Mods::NONE,
                text: None,
                pressed: false,
                repeat: false
            }]
        );
    }

    #[test]
    fn kitty_shift_uses_shifted_key_or_uppercase() {
        // shift+a with alternate key reported
        assert_eq!(
            feed(b"\x1b[97:65;2u"),
            vec![key(Key::Char('a'), Mods::SHIFT, Some("A"))]
        );
        // shift+1 -> '!'
        assert_eq!(
            feed(b"\x1b[49:33;2u"),
            vec![key(Key::Char('1'), Mods::SHIFT, Some("!"))]
        );
        // no alternate key: uppercase ascii
        assert_eq!(
            feed(b"\x1b[112;2u"),
            vec![key(Key::Char('p'), Mods::SHIFT, Some("P"))]
        );
    }

    #[test]
    fn kitty_ctrl_and_alt_have_no_text() {
        assert_eq!(
            feed(b"\x1b[99;5u"),
            vec![key(Key::Char('c'), Mods::CTRL, None)]
        );
        assert_eq!(
            feed(b"\x1b[120;3u"),
            vec![key(Key::Char('x'), Mods::ALT, None)]
        );
        // ctrl+shift+p: modifier 6
        assert_eq!(
            feed(b"\x1b[112;6u"),
            vec![key(
                Key::Char('p'),
                Mods {
                    shift: true,
                    ctrl: true,
                    ..Mods::NONE
                },
                None
            )]
        );
    }

    #[test]
    fn kitty_text_field_wins() {
        // key 97 with explicit text codepoints "ä" (228)
        assert_eq!(
            feed(b"\x1b[97;1;228u"),
            vec![key(Key::Char('a'), Mods::NONE, Some("ä"))]
        );
    }

    #[test]
    fn kitty_lock_modifiers_are_masked() {
        // caps lock (64) + shift (1): param 66
        assert_eq!(
            feed(b"\x1b[97:65;66u"),
            vec![key(Key::Char('a'), Mods::SHIFT, Some("A"))]
        );
        // num lock alone: param 129 -> no mods
        assert_eq!(
            feed(b"\x1b[57399;129u"),
            vec![key(Key::Char('0'), Mods::NONE, Some("0"))]
        );
    }

    #[test]
    fn kitty_functional_keys() {
        assert_eq!(feed(b"\x1b[27u"), vec![key(Key::Escape, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[13u"), vec![key(Key::Enter, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[13;5u"), vec![key(Key::Enter, Mods::CTRL, None)]);
        assert_eq!(feed(b"\x1b[9u"), vec![key(Key::Tab, Mods::NONE, None)]);
        assert_eq!(
            feed(b"\x1b[127u"),
            vec![key(Key::Backspace, Mods::NONE, None)]
        );
        assert_eq!(
            feed(b"\x1b[57348u"),
            vec![key(Key::Insert, Mods::NONE, None)]
        );
        assert_eq!(
            feed(b"\x1b[57349u"),
            vec![key(Key::Delete, Mods::NONE, None)]
        );
        assert_eq!(feed(b"\x1b[57350u"), vec![key(Key::Left, Mods::NONE, None)]);
        assert_eq!(
            feed(b"\x1b[57351u"),
            vec![key(Key::Right, Mods::NONE, None)]
        );
        assert_eq!(feed(b"\x1b[57352u"), vec![key(Key::Up, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[57353u"), vec![key(Key::Down, Mods::NONE, None)]);
        assert_eq!(
            feed(b"\x1b[57354u"),
            vec![key(Key::PageUp, Mods::NONE, None)]
        );
        assert_eq!(
            feed(b"\x1b[57355u"),
            vec![key(Key::PageDown, Mods::NONE, None)]
        );
        assert_eq!(feed(b"\x1b[57356u"), vec![key(Key::Home, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[57357u"), vec![key(Key::End, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[57364u"), vec![key(Key::F(1), Mods::NONE, None)]);
        assert_eq!(
            feed(b"\x1b[57375u"),
            vec![key(Key::F(12), Mods::NONE, None)]
        );
        // space is text
        assert_eq!(
            feed(b"\x1b[32u"),
            vec![key(Key::Char(' '), Mods::NONE, Some(" "))]
        );
        // left shift press: Other, no text
        assert_eq!(
            feed(b"\x1b[57441;2u"),
            vec![key(Key::Other(57441), Mods::SHIFT, None)]
        );
    }

    #[test]
    fn legacy_keys() {
        assert_eq!(feed(b"\x1b[A"), vec![key(Key::Up, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[B"), vec![key(Key::Down, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[C"), vec![key(Key::Right, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[D"), vec![key(Key::Left, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[1;5A"), vec![key(Key::Up, Mods::CTRL, None)]);
        assert_eq!(feed(b"\x1b[1;2D"), vec![key(Key::Left, Mods::SHIFT, None)]);
        assert_eq!(feed(b"\x1b[H"), vec![key(Key::Home, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[F"), vec![key(Key::End, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[2~"), vec![key(Key::Insert, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[3~"), vec![key(Key::Delete, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[5~"), vec![key(Key::PageUp, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[6~"), vec![key(Key::PageDown, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[3;5~"), vec![key(Key::Delete, Mods::CTRL, None)]);
        assert_eq!(feed(b"\x1b[Z"), vec![key(Key::Tab, Mods::SHIFT, None)]);
        assert_eq!(feed(b"\x1bOA"), vec![key(Key::Up, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1b[15~"), vec![key(Key::F(5), Mods::NONE, None)]);
    }

    #[test]
    fn fallback_text_ctrl_and_alt() {
        assert_eq!(feed(b"a"), vec![key(Key::Char('a'), Mods::NONE, Some("a"))]);
        assert_eq!(
            feed("é".as_bytes()),
            vec![key(Key::Char('é'), Mods::NONE, Some("é"))]
        );
        assert_eq!(feed(b"\x03"), vec![key(Key::Char('c'), Mods::CTRL, None)]);
        assert_eq!(feed(b"\x01"), vec![key(Key::Char('a'), Mods::CTRL, None)]);
        assert_eq!(feed(b"\r"), vec![key(Key::Enter, Mods::NONE, None)]);
        assert_eq!(feed(b"\t"), vec![key(Key::Tab, Mods::NONE, None)]);
        assert_eq!(feed(b"\x7f"), vec![key(Key::Backspace, Mods::NONE, None)]);
        assert_eq!(feed(b"\x1bx"), vec![key(Key::Char('x'), Mods::ALT, None)]);
        assert_eq!(
            feed(b"ab"),
            vec![
                key(Key::Char('a'), Mods::NONE, Some("a")),
                key(Key::Char('b'), Mods::NONE, Some("b"))
            ]
        );
    }

    #[test]
    fn utf8_split_across_feeds() {
        let mut p = Parser::new(true, 16, 34);
        let bytes = "é".as_bytes();
        assert!(p.feed(&bytes[..1]).is_empty());
        assert!(p.has_pending());
        assert_eq!(
            p.feed(&bytes[1..]),
            vec![key(Key::Char('é'), Mods::NONE, Some("é"))]
        );
        assert!(!p.has_pending());
    }

    #[test]
    fn sgr_mouse_pixels() {
        assert_eq!(
            feed(b"\x1b[<0;101;201M"),
            vec![Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
                x: 100,
                y: 200,
                mods: Mods::NONE
            }]
        );
        assert_eq!(
            feed(b"\x1b[<0;101;201m"),
            vec![Event::MouseButton {
                button: MouseButton::Left,
                pressed: false,
                x: 100,
                y: 200,
                mods: Mods::NONE
            }]
        );
        assert_eq!(
            feed(b"\x1b[<2;1;1M"),
            vec![Event::MouseButton {
                button: MouseButton::Right,
                pressed: true,
                x: 0,
                y: 0,
                mods: Mods::NONE
            }]
        );
        assert_eq!(
            feed(b"\x1b[<1;5;5M"),
            vec![Event::MouseButton {
                button: MouseButton::Middle,
                pressed: true,
                x: 4,
                y: 4,
                mods: Mods::NONE
            }]
        );
        // motion with no button: 32 + 3
        assert_eq!(
            feed(b"\x1b[<35;11;21M"),
            vec![Event::MouseMove {
                x: 10,
                y: 20,
                mods: Mods::NONE
            }]
        );
        // drag with left button: 32 + 0
        assert_eq!(
            feed(b"\x1b[<32;11;21M"),
            vec![Event::MouseMove {
                x: 10,
                y: 20,
                mods: Mods::NONE
            }]
        );
        // shift+click: 4
        assert_eq!(
            feed(b"\x1b[<4;11;21M"),
            vec![Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
                x: 10,
                y: 20,
                mods: Mods::SHIFT
            }]
        );
        // ctrl+drag: 16 + 32
        assert_eq!(
            feed(b"\x1b[<48;11;21M"),
            vec![Event::MouseMove {
                x: 10,
                y: 20,
                mods: Mods::CTRL
            }]
        );
    }

    #[test]
    fn sgr_wheel() {
        assert_eq!(
            feed(b"\x1b[<64;11;21M"),
            vec![Event::Wheel {
                dx: 0,
                dy: 1,
                x: 10,
                y: 20,
                mods: Mods::NONE
            }]
        );
        assert_eq!(
            feed(b"\x1b[<65;11;21M"),
            vec![Event::Wheel {
                dx: 0,
                dy: -1,
                x: 10,
                y: 20,
                mods: Mods::NONE
            }]
        );
        assert_eq!(
            feed(b"\x1b[<66;11;21M"),
            vec![Event::Wheel {
                dx: 1,
                dy: 0,
                x: 10,
                y: 20,
                mods: Mods::NONE
            }]
        );
        assert_eq!(
            feed(b"\x1b[<67;11;21M"),
            vec![Event::Wheel {
                dx: -1,
                dy: 0,
                x: 10,
                y: 20,
                mods: Mods::NONE
            }]
        );
        // shift+wheel down: 65 + 4
        assert_eq!(
            feed(b"\x1b[<69;11;21M"),
            vec![Event::Wheel {
                dx: 0,
                dy: -1,
                x: 10,
                y: 20,
                mods: Mods::SHIFT
            }]
        );
    }

    #[test]
    fn sgr_mouse_cells_convert_to_pixel_centers() {
        let mut p = Parser::new(false, 16, 34);
        assert_eq!(
            p.feed(b"\x1b[<0;3;2M"),
            vec![Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
                x: 2 * 16 + 8,
                y: 34 + 17,
                mods: Mods::NONE
            }]
        );
    }

    #[test]
    fn focus_and_paste() {
        assert_eq!(feed(b"\x1b[I"), vec![Event::Focus(true)]);
        assert_eq!(feed(b"\x1b[O"), vec![Event::Focus(false)]);
        assert_eq!(
            feed(b"\x1b[200~hello\nworld\x1b[201~"),
            vec![Event::Paste("hello\nworld".into())]
        );
        // Paste split across feeds
        let mut p = Parser::new(true, 1, 1);
        assert!(p.feed(b"\x1b[200~par").is_empty());
        assert_eq!(
            p.feed(b"tial\x1b[201~x"),
            vec![
                Event::Paste("partial".into()),
                key(Key::Char('x'), Mods::NONE, Some("x"))
            ]
        );
    }

    #[test]
    fn incomplete_then_complete_and_flush() {
        let mut p = Parser::new(true, 1, 1);
        assert!(p.feed(b"\x1b[<0;10").is_empty());
        assert!(p.has_pending());
        assert_eq!(
            p.feed(b";20M"),
            vec![Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
                x: 9,
                y: 19,
                mods: Mods::NONE
            }]
        );
        // lone ESC then flush -> Escape key
        assert!(p.feed(b"\x1b").is_empty());
        assert_eq!(p.flush(), vec![key(Key::Escape, Mods::NONE, None)]);
        assert!(!p.has_pending());
        // ESC ESC -> two escapes eventually
        assert_eq!(
            p.feed(b"\x1b\x1b"),
            vec![key(Key::Escape, Mods::NONE, None)]
        );
        assert_eq!(p.flush(), vec![key(Key::Escape, Mods::NONE, None)]);
    }

    #[test]
    fn mixed_stream_in_one_feed() {
        let evs = feed(b"\x1b[113u\x1b[<35;5;5Mq\x1b[I");
        assert_eq!(evs.len(), 4);
        assert!(matches!(
            evs[0],
            Event::Key {
                key: Key::Char('q'),
                ..
            }
        ));
        assert!(matches!(evs[1], Event::MouseMove { .. }));
        assert!(matches!(
            evs[2],
            Event::Key {
                key: Key::Char('q'),
                ..
            }
        ));
        assert_eq!(evs[3], Event::Focus(true));
    }

    #[test]
    fn terminal_replies_are_skipped() {
        // Late probe replies (APC graphics OK, DA) must not produce key events.
        assert!(feed(b"\x1b_Gi=31;OK\x1b\\").is_empty());
        assert_eq!(
            feed(b"\x1b[?62;22c"),
            vec![Event::Unknown(b"\x1b[?62;22c".to_vec())]
        );
        assert!(feed(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\").is_empty());
        assert!(feed(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x07").is_empty());
    }
}
