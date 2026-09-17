use crate::tui::terminal_events::parser::Parser;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use termwiz::input::{InputEvent, Modifiers, MouseButtons, MouseEvent};
use windows_sys::Win32::System::Console::*;

#[derive(Default)]
pub(super) struct Decoder {
    parser: Parser,
}

impl Decoder {
    pub(super) fn decode(&mut self, records: &[INPUT_RECORD], more: bool) -> Vec<Event> {
        let mut events = Vec::new();
        for record in records {
            match u32::from(record.EventType) {
                KEY_EVENT => {
                    let key = unsafe { record.Event.KeyEvent };
                    let unit = unsafe { key.uChar.UnicodeChar };
                    let nul_character = unit == 0
                        && (key.wVirtualKeyCode == 0
                            || (matches!(key.wVirtualKeyCode, 0x20 | 0x32)
                                && key.dwControlKeyState
                                    & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED)
                                    != 0));
                    if unit != 0 || nul_character {
                        // VT characters are bytes of the terminal protocol, not
                        // virtual keys. Preserve control characters and UTF-16
                        // pairs; key-up records must not duplicate paste text.
                        // Classic conhost emits Alt+numpad text on VK_MENU
                        // release only. Other character releases duplicate the
                        // press and must not re-enter the ANSI parser.
                        const VK_MENU: u16 = 0x12;
                        if key.bKeyDown != 0 || key.wVirtualKeyCode == VK_MENU {
                            for _ in 0..key.wRepeatCount {
                                self.parser.push_utf16(unit);
                            }
                        }
                    } else if let Some(code) = virtual_key(key.wVirtualKeyCode) {
                        events.extend(self.parser.parse(&[], /*more*/ true));
                        let mut modifiers = KeyModifiers::empty();
                        if key.dwControlKeyState & SHIFT_PRESSED != 0 {
                            modifiers.insert(KeyModifiers::SHIFT);
                        }
                        if key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
                            modifiers.insert(KeyModifiers::CONTROL);
                        }
                        if key.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
                            modifiers.insert(KeyModifiers::ALT);
                        }
                        let code =
                            if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
                                KeyCode::BackTab
                            } else {
                                code
                            };
                        let kind = if key.bKeyDown != 0 {
                            KeyEventKind::Press
                        } else {
                            KeyEventKind::Release
                        };
                        for _ in 0..key.wRepeatCount {
                            events.push(Event::Key(KeyEvent::new_with_kind(code, modifiers, kind)));
                        }
                    }
                }
                MOUSE_EVENT => {
                    events.extend(self.parser.parse(&[], /*more*/ true));
                    let mouse = unsafe { record.Event.MouseEvent };
                    let mut buttons = MouseButtons::NONE;
                    for (mask, button) in [
                        (FROM_LEFT_1ST_BUTTON_PRESSED, MouseButtons::LEFT),
                        (RIGHTMOST_BUTTON_PRESSED, MouseButtons::RIGHT),
                        (FROM_LEFT_2ND_BUTTON_PRESSED, MouseButtons::MIDDLE),
                    ] {
                        if mouse.dwButtonState & mask != 0 {
                            buttons.insert(button);
                        }
                    }
                    if mouse.dwEventFlags & MOUSE_WHEELED != 0 {
                        buttons.insert(MouseButtons::VERT_WHEEL);
                    }
                    if mouse.dwEventFlags & MOUSE_HWHEELED != 0 {
                        buttons.insert(MouseButtons::HORZ_WHEEL);
                    }
                    if (mouse.dwButtonState >> 16) as i16 > 0 {
                        buttons.insert(MouseButtons::WHEEL_POSITIVE);
                    }
                    let mut modifiers = Modifiers::NONE;
                    if mouse.dwControlKeyState & SHIFT_PRESSED != 0 {
                        modifiers.insert(Modifiers::SHIFT);
                    }
                    if mouse.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
                        modifiers.insert(Modifiers::CTRL);
                    }
                    if mouse.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
                        modifiers.insert(Modifiers::ALT);
                    }
                    if let Some(event) = self.parser.convert(InputEvent::Mouse(MouseEvent {
                        x: mouse.dwMousePosition.X.max(0) as u16,
                        y: mouse.dwMousePosition.Y.max(0) as u16,
                        mouse_buttons: buttons,
                        modifiers,
                    })) {
                        events.push(event);
                    }
                }
                WINDOW_BUFFER_SIZE_EVENT => {
                    events.extend(self.parser.parse(&[], /*more*/ true));
                    let size = unsafe { record.Event.WindowBufferSizeEvent.dwSize };
                    events.push(Event::Resize(size.X.max(0) as u16, size.Y.max(0) as u16));
                }
                FOCUS_EVENT => events.push(if unsafe { record.Event.FocusEvent.bSetFocus } != 0 {
                    Event::FocusGained
                } else {
                    Event::FocusLost
                }),
                // MENU_EVENT and reserved future console records carry no TUI input.
                _ => {}
            }
        }
        // Resolve a lone Esc at the end of the available input batch. The
        // parser retains bracketed paste payloads across queue-empty boundaries.
        events.extend(self.parser.parse(&[], more));
        events
    }
}

fn virtual_key(key: u16) -> Option<KeyCode> {
    // Win32 virtual-key codes, used only for records with no VT character.
    Some(match key {
        0x08 => KeyCode::Backspace,
        0x09 => KeyCode::Tab,
        0x0d => KeyCode::Enter,
        0x1b => KeyCode::Esc,
        0x21 => KeyCode::PageUp,
        0x22 => KeyCode::PageDown,
        0x23 => KeyCode::End,
        0x24 => KeyCode::Home,
        0x25 => KeyCode::Left,
        0x26 => KeyCode::Up,
        0x27 => KeyCode::Right,
        0x28 => KeyCode::Down,
        0x2d => KeyCode::Insert,
        0x2e => KeyCode::Delete,
        0x70..=0x87 => KeyCode::F((key - 0x70 + 1) as u8),
        _ => return None,
    })
}

#[cfg(test)]
#[path = "windows_input_records_tests.rs"]
mod tests;
