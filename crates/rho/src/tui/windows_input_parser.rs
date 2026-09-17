//! Convert the published termwiz ANSI parser into Rho's crossterm events.
//! Kept platform-independent so the exact shipped parser is tested on every host.
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use termwiz::input::{InputEvent, InputParser, KeyCode as Key, Modifiers, MouseButtons};

#[derive(Default)]
pub(super) struct Parser {
    parser: InputParser,
    mouse: Option<MouseButton>,
    high_surrogate: Option<u16>,
    bytes: Vec<u8>,
    pasting: bool,
}

impl Parser {
    pub(super) fn push_utf16(&mut self, unit: u16) {
        if (0xd800..=0xdbff).contains(&unit) {
            self.high_surrogate = Some(unit);
            return;
        }
        let ch = if let Some(high) = self.high_surrogate.take() {
            char::decode_utf16([high, unit]).next().and_then(Result::ok)
        } else {
            char::from_u32(u32::from(unit))
        };
        if let Some(ch) = ch {
            let mut bytes = [0; 4];
            self.bytes
                .extend_from_slice(ch.encode_utf8(&mut bytes).as_bytes());
        }
    }

    pub(super) fn parse(&mut self, bytes: &[u8], more: bool) -> Vec<Event> {
        self.bytes.extend_from_slice(bytes);
        let mut bytes = std::mem::take(&mut self.bytes);
        // termwiz recognizes SGR mouse reports as complete CSI sequences but
        // does not retain an incomplete SGR report. Keep a trailing partial
        // CSI here so console batch boundaries cannot turn mouse into text.
        // Keep its leading ESC too when more input is known to follow, or
        // while pasting, so our paste state and termwiz see the same markers.
        let end = bytes
            .iter()
            .rposition(|byte| *byte == 0x1b)
            .filter(|&start| {
                let tail = &bytes[start..];
                (tail == b"\x1b" && (more || self.pasting))
                    || (tail.starts_with(b"\x1b[")
                        && !tail[2..].iter().any(|byte| (0x40..=0x7e).contains(byte)))
            })
            .unwrap_or(bytes.len());
        let mut events = Vec::new();
        let mut segment = 0;
        let mut cursor = 0;
        while cursor < end {
            let tail = &bytes[cursor..end];
            // In VT input mode conhost sends focus as CSI I/O, not native
            // FOCUS_EVENT records. termwiz has no focus event variant.
            let focus = [
                (b"\x1b[I", Event::FocusGained),
                (b"\x1b[O", Event::FocusLost),
            ]
            .into_iter()
            .find(|(sequence, _)| !self.pasting && tail.starts_with(*sequence));
            if let Some((sequence, event)) = focus {
                self.parse_ansi(&bytes[segment..cursor], /*more*/ true, &mut events);
                events.push(event);
                cursor += sequence.len();
                segment = cursor;
                continue;
            }
            let marker = if self.pasting {
                b"\x1b[201~"
            } else {
                b"\x1b[200~"
            };
            if tail.starts_with(marker) {
                cursor += marker.len();
                self.parse_ansi(&bytes[segment..cursor], /*more*/ true, &mut events);
                segment = cursor;
                self.pasting = !self.pasting;
                continue;
            }
            // Match crossterm's raw-input identities. termwiz otherwise turns
            // Ctrl+J into Enter (which would submit Rho's editor), Ctrl+H into
            // Backspace, and leaves NUL/control punctuation unmodified. CSI u
            // supplies the intended key without bypassing its Alt-prefix state.
            // Never rewrite bytes inside paste payloads.
            let control = if self.pasting {
                None
            } else {
                match bytes[cursor] {
                    0 => Some(b' '),
                    8 => Some(b'h'),
                    10 => Some(b'j'),
                    byte @ 0x1c..=0x1f => Some(byte - 0x1c + b'4'),
                    _ => None,
                }
            };
            if let Some(control) = control {
                self.parse_ansi(&bytes[segment..cursor], /*more*/ true, &mut events);
                self.parse_ansi(
                    format!("\x1b[{control};5u").as_bytes(),
                    /*more*/ true,
                    &mut events,
                );
                segment = cursor + 1;
            }
            cursor += 1;
        }
        self.parse_ansi(&bytes[segment..end], more || end < bytes.len(), &mut events);
        bytes.drain(..end);
        self.bytes = bytes;
        events
    }

    fn parse_ansi(&mut self, bytes: &[u8], more: bool, events: &mut Vec<Event>) {
        let parsed = self.parser.parse_as_vec(bytes, more);
        events.extend(parsed.into_iter().filter_map(|mut event| {
            if let InputEvent::Mouse(mouse) = &mut event {
                // SGR coordinates are one-based; native console records
                // already use the zero-based coordinates convert expects.
                mouse.x = mouse.x.saturating_sub(1);
                mouse.y = mouse.y.saturating_sub(1);
            }
            self.convert(event)
        }));
    }

    pub(super) fn convert(&mut self, event: InputEvent) -> Option<Event> {
        match event {
            InputEvent::Paste(text) => Some(Event::Paste(text)),
            InputEvent::Key(key) => key_code(key.key, key.modifiers)
                .map(|code| Event::Key(KeyEvent::new(code, modifiers(key.modifiers)))),
            InputEvent::Mouse(mouse) => {
                let buttons = mouse.mouse_buttons;
                let button = if buttons.contains(MouseButtons::LEFT) {
                    Some(MouseButton::Left)
                } else if buttons.contains(MouseButtons::RIGHT) {
                    Some(MouseButton::Right)
                } else if buttons.contains(MouseButtons::MIDDLE) {
                    Some(MouseButton::Middle)
                } else {
                    None
                };
                let kind = if buttons.contains(MouseButtons::VERT_WHEEL) {
                    if buttons.contains(MouseButtons::WHEEL_POSITIVE) {
                        MouseEventKind::ScrollUp
                    } else {
                        MouseEventKind::ScrollDown
                    }
                } else if buttons.contains(MouseButtons::HORZ_WHEEL) {
                    if buttons.contains(MouseButtons::WHEEL_POSITIVE) {
                        MouseEventKind::ScrollRight
                    } else {
                        MouseEventKind::ScrollLeft
                    }
                } else {
                    let kind = match (self.mouse, button) {
                        (Some(previous), None) => MouseEventKind::Up(previous),
                        (Some(previous), Some(current)) if previous == current => {
                            MouseEventKind::Drag(current)
                        }
                        (_, Some(current)) => MouseEventKind::Down(current),
                        (None, None) => MouseEventKind::Moved,
                    };
                    self.mouse = button;
                    kind
                };
                Some(Event::Mouse(MouseEvent {
                    kind,
                    column: mouse.x,
                    row: mouse.y,
                    modifiers: modifiers(mouse.modifiers),
                }))
            }
            InputEvent::Resized { cols, rows } => {
                Some(Event::Resize(cols.try_into().ok()?, rows.try_into().ok()?))
            }
            // Pixel reporting is never requested. Wake is a termwiz-terminal
            // control event, not terminal input.
            InputEvent::PixelMouse(_) | InputEvent::Wake => None,
        }
    }
}

fn modifiers(mods: Modifiers) -> KeyModifiers {
    let mut result = KeyModifiers::empty();
    for (source, target) in [
        (Modifiers::SHIFT, KeyModifiers::SHIFT),
        (Modifiers::ALT, KeyModifiers::ALT),
        (Modifiers::CTRL, KeyModifiers::CONTROL),
        (Modifiers::SUPER, KeyModifiers::SUPER),
    ] {
        if mods.contains(source) {
            result.insert(target);
        }
    }
    result
}

fn key_code(key: Key, mods: Modifiers) -> Option<KeyCode> {
    Some(match key {
        Key::Char(ch) => KeyCode::Char(ch),
        Key::Backspace => KeyCode::Backspace,
        Key::Tab if mods.contains(Modifiers::SHIFT) => KeyCode::BackTab,
        Key::Tab => KeyCode::Tab,
        Key::Enter => KeyCode::Enter,
        Key::Escape => KeyCode::Esc,
        Key::LeftArrow | Key::ApplicationLeftArrow => KeyCode::Left,
        Key::RightArrow | Key::ApplicationRightArrow => KeyCode::Right,
        Key::UpArrow | Key::ApplicationUpArrow => KeyCode::Up,
        Key::DownArrow | Key::ApplicationDownArrow => KeyCode::Down,
        Key::Home | Key::KeyPadHome => KeyCode::Home,
        Key::End | Key::KeyPadEnd => KeyCode::End,
        Key::PageUp | Key::KeyPadPageUp => KeyCode::PageUp,
        Key::PageDown | Key::KeyPadPageDown => KeyCode::PageDown,
        Key::Insert => KeyCode::Insert,
        Key::Delete => KeyCode::Delete,
        Key::Function(number) => KeyCode::F(number),
        Key::CapsLock => KeyCode::CapsLock,
        Key::NumLock => KeyCode::NumLock,
        Key::ScrollLock => KeyCode::ScrollLock,
        Key::PrintScreen => KeyCode::PrintScreen,
        Key::Pause => KeyCode::Pause,
        Key::KeyPadBegin => KeyCode::KeypadBegin,
        // The ANSI parser does not emit the following Win32-only keys or
        // internal delimiters as application key events.
        Key::Hyper
        | Key::Super
        | Key::Meta
        | Key::Cancel
        | Key::Clear
        | Key::Shift
        | Key::LeftShift
        | Key::RightShift
        | Key::Control
        | Key::LeftControl
        | Key::RightControl
        | Key::Alt
        | Key::LeftAlt
        | Key::RightAlt
        | Key::Menu
        | Key::LeftMenu
        | Key::RightMenu
        | Key::Select
        | Key::Print
        | Key::Execute
        | Key::Help
        | Key::LeftWindows
        | Key::RightWindows
        | Key::Applications
        | Key::Sleep
        | Key::Numpad0
        | Key::Numpad1
        | Key::Numpad2
        | Key::Numpad3
        | Key::Numpad4
        | Key::Numpad5
        | Key::Numpad6
        | Key::Numpad7
        | Key::Numpad8
        | Key::Numpad9
        | Key::Multiply
        | Key::Add
        | Key::Separator
        | Key::Subtract
        | Key::Decimal
        | Key::Divide
        | Key::Copy
        | Key::Cut
        | Key::Paste
        | Key::BrowserBack
        | Key::BrowserForward
        | Key::BrowserRefresh
        | Key::BrowserStop
        | Key::BrowserSearch
        | Key::BrowserFavorites
        | Key::BrowserHome
        | Key::VolumeMute
        | Key::VolumeDown
        | Key::VolumeUp
        | Key::MediaNextTrack
        | Key::MediaPrevTrack
        | Key::MediaStop
        | Key::MediaPlayPause
        | Key::InternalPasteStart
        | Key::InternalPasteEnd => return None,
    })
}

#[cfg(test)]
#[path = "windows_input_parser_tests.rs"]
mod tests;
