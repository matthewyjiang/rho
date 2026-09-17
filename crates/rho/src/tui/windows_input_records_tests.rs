use super::*;
use pretty_assertions::assert_eq;

// Covers: conhost Alt+numpad text arrives on Alt release, while ordinary key
// releases must not duplicate input. Owner: Windows console-record decoding.
#[test]
fn alt_code_release_is_text_but_ordinary_release_is_not() {
    // VK_MENU and VK_A from the Win32 virtual-key table.
    for (virtual_key, key_down, expected) in [
        (
            0x12,
            false,
            vec![Event::Key(KeyEvent::new(
                KeyCode::Char('é'),
                KeyModifiers::NONE,
            ))],
        ),
        (0x41, false, vec![]),
        (
            0x41,
            true,
            vec![Event::Key(KeyEvent::new(
                KeyCode::Char('é'),
                KeyModifiers::NONE,
            ))],
        ),
    ] {
        let mut record = INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            ..Default::default()
        };
        record.Event.KeyEvent = KEY_EVENT_RECORD {
            bKeyDown: i32::from(key_down),
            wRepeatCount: 1,
            wVirtualKeyCode: virtual_key,
            uChar: KEY_EVENT_RECORD_0 {
                UnicodeChar: 'é' as u16,
            },
            ..Default::default()
        };
        assert_eq!(
            Decoder::default().decode(&[record], /*more*/ false),
            expected
        );
    }
}

// Covers: NUL from Ctrl+Space/Ctrl+2 is text input, not an empty modifier
// record. Owner: Windows console-record filtering, before ANSI parsing.
#[test]
fn nul_key_records_reach_the_control_key_parser() {
    // VT transport uses VK=0; legacy-shaped hosts can retain VK_SPACE/VK_2.
    for virtual_key in [0, 0x20, 0x32] {
        let mut record = INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            ..Default::default()
        };
        record.Event.KeyEvent = KEY_EVENT_RECORD {
            bKeyDown: 1,
            wRepeatCount: 1,
            wVirtualKeyCode: virtual_key,
            dwControlKeyState: LEFT_CTRL_PRESSED,
            ..Default::default()
        };
        assert_eq!(
            Decoder::default().decode(&[record], /*more*/ false),
            vec![Event::Key(KeyEvent::new(
                KeyCode::Char(' '),
                KeyModifiers::CONTROL
            ))]
        );
    }
}
