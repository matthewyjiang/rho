use super::*;
use pretty_assertions::assert_eq;

// Covers: the packaged Windows parser must keep embedded Enter/control bytes
// and split UTF-16 pairs inside one paste instead of submitting the editor.
// Owner: terminal input decoding. PTY cannot choose console record boundaries.
#[test]
fn paste_survives_console_batches_and_utf16_pairs() {
    let payload = "first\r\n東京 🦀\t\x03\x00\x08last";
    let start = b"\x1b[200~";
    let end = b"\x1b[201~";
    for start_split in 1..=start.len() {
        for end_split in 1..=end.len() {
            let mut parser = Parser::default();
            // A lone ESC outside paste is ambiguous only when the queue is
            // empty. Inside paste even a queue-empty split must preserve it.
            let mut events = parser.parse(&start[..start_split], /*more*/ true);
            events.extend(parser.parse(&start[start_split..], /*more*/ false));
            assert_eq!(events, Vec::<Event>::new(), "start split {start_split}");
            for unit in payload.encode_utf16() {
                parser.push_utf16(unit);
                assert_eq!(parser.parse(&[], /*more*/ false), Vec::<Event>::new());
            }
            let mut events = parser.parse(&end[..end_split], /*more*/ false);
            events.extend(parser.parse(&end[end_split..], /*more*/ false));
            assert_eq!(
                events,
                vec![Event::Paste(payload.to_owned())],
                "start split {start_split}, end split {end_split}"
            );
            assert_eq!(
                parser.parse(b"\n\r", /*more*/ false),
                vec![
                    Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
                    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                ]
            );
        }
    }
}

// Covers: enabling paste must not turn navigation, interrupt, or mouse input
// into literal escape sequences. Owner: terminal input protocol conversion.
#[test]
fn vt_events_preserve_key_and_mouse_semantics() {
    let cases: &[(&[u8], Event)] = &[
        (
            b"\x00",
            Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
        ),
        (
            b"\x08",
            Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
        ),
        (
            b"\x0a",
            Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
        ),
        (
            b"\x7f",
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        ),
        (
            b"\x1b",
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        ),
        (
            b"\x1b[A",
            Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        ),
        (
            b"\x1b[Z",
            Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        ),
        (
            b"\x03",
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ),
        (
            b"\x1b[<64;4;5M",
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 3,
                row: 4,
                modifiers: KeyModifiers::NONE,
            }),
        ),
    ];
    for (bytes, expected) in cases {
        assert_eq!(
            Parser::default().parse(bytes, /*more*/ false),
            vec![expected.clone()]
        );
        // A split immediately after ESC is inherently ambiguous at an empty
        // console queue. Once CSI starts, every later split must be lossless.
        if bytes.starts_with(b"\x1b[") {
            for split in 2..bytes.len() {
                let mut parser = Parser::default();
                let mut events = parser.parse(&bytes[..split], /*more*/ false);
                events.extend(parser.parse(&bytes[split..], /*more*/ false));
                assert_eq!(events, vec![expected.clone()]);
            }
        }
    }
}
