//! Raw key, paste, and mouse encodings for PTY injection.

/// Named keys that map to common terminal byte sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Text(String),
    Enter,
    Esc,
    Tab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    CtrlEnd,
    Ctrl(char),
    Alt(char),
    AltUp,
}

/// SGR mouse button identifiers used by the harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    WheelUp = 64,
    WheelDown = 65,
}

/// Encode a named key into terminal input bytes.
pub fn encode_key(key: &Key) -> Vec<u8> {
    match key {
        Key::Char(ch) => {
            let mut buf = [0u8; 4];
            ch.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        Key::Text(text) => text.as_bytes().to_vec(),
        Key::Enter => b"\r".to_vec(),
        Key::Esc => b"\x1b".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::CtrlEnd => b"\x1b[1;5F".to_vec(),
        Key::Ctrl(ch) => {
            let lower = ch.to_ascii_lowercase();
            if lower.is_ascii_lowercase() {
                vec![(lower as u8) & 0x1f]
            } else {
                encode_key(&Key::Char(*ch))
            }
        }
        Key::Alt(ch) => {
            let mut out = vec![0x1b];
            out.extend(encode_key(&Key::Char(*ch)));
            out
        }
        Key::AltUp => b"\x1b[1;3A".to_vec(),
    }
}

/// Encode bracketed-paste content.
pub fn encode_paste(text: &str) -> Vec<u8> {
    let mut out = b"\x1b[200~".to_vec();
    out.extend(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

/// Encode an SGR mouse event. Columns and rows are 1-based.
pub fn encode_sgr_mouse(button: MouseButton, col: u16, row: u16, press: bool) -> Vec<u8> {
    let suffix = if press { b'M' } else { b'm' };
    format!(
        "\x1b[<{};{};{}{}",
        button as u16,
        col.max(1),
        row.max(1),
        suffix as char
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn encodes_ctrl_c_and_arrows() {
        assert_eq!(encode_key(&Key::Ctrl('c')), b"\x03");
        assert_eq!(encode_key(&Key::Up), b"\x1b[A");
        assert_eq!(encode_key(&Key::PageDown), b"\x1b[6~");
        assert_eq!(encode_key(&Key::AltUp), b"\x1b[1;3A");
    }

    #[test]
    fn encodes_bracketed_paste() {
        assert_eq!(
            encode_paste("hello\nworld"),
            b"\x1b[200~hello\nworld\x1b[201~"
        );
    }

    #[test]
    fn encodes_sgr_mouse_press_and_release() {
        assert_eq!(
            encode_sgr_mouse(MouseButton::Left, 3, 4, true),
            b"\x1b[<0;3;4M"
        );
        assert_eq!(
            encode_sgr_mouse(MouseButton::WheelUp, 1, 1, false),
            b"\x1b[<64;1;1m"
        );
    }
}
