//! Incremental extraction of the top-level script string from tool arguments.
//!
//! Only new argument bytes are scanned. Escapes (including surrogate pairs)
//! stay pending across provider deltas; source rows are assembled once rather
//! than reparsing and splitting the entire argument buffer on every update.
//! This is a preview, not validation: execution still uses the completed JSON.

#[derive(Clone, Copy, Debug)]
enum StringRole {
    Key,
    Script,
    Other,
}

#[derive(Clone, Copy, Debug, Default)]
enum Escape {
    #[default]
    None,
    Slash,
    Unicode {
        value: u16,
        digits: u8,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct ScriptStream {
    consumed: usize,
    depth: usize,
    expecting_key: bool,
    key: String,
    key_buffer: String,
    role: Option<StringRole>,
    escape: Escape,
    high_surrogate: Option<u16>,
    invalid: bool,
    lines: Vec<String>,
    visible_end: Option<(usize, usize)>,
    revision: usize,
}

impl ScriptStream {
    /// Consume only the unobserved suffix, including arguments received before
    /// the provider named the tool. Returns whether decoded source changed.
    pub(super) fn update(&mut self, arguments: &str) -> bool {
        let revision = self.revision;
        for character in arguments[self.consumed..].chars() {
            if self.invalid {
                break;
            }
            self.consume(character);
        }
        self.consumed = arguments.len();
        self.revision != revision
    }

    /// Full source history, with the same trailing-whitespace policy as cards
    /// built from completed arguments. No full-source split or JSON parse.
    pub(super) fn lines(&self) -> Vec<String> {
        let Some((last, end)) = self.visible_end else {
            return Vec::new();
        };
        let mut lines = self.lines[..=last].to_vec();
        lines[last].truncate(end);
        lines
    }

    fn consume(&mut self, character: char) {
        if let Some(role) = self.role {
            match self.escape {
                Escape::Slash => {
                    self.escape = Escape::None;
                    let decoded = match character {
                        '"' => '"',
                        '\\' => '\\',
                        '/' => '/',
                        'b' => '\u{0008}',
                        'f' => '\u{000c}',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'u' => {
                            self.escape = Escape::Unicode {
                                value: 0,
                                digits: 0,
                            };
                            return;
                        }
                        _ => {
                            self.invalid = true;
                            return;
                        }
                    };
                    self.push(role, decoded);
                }
                Escape::Unicode { value, digits } => {
                    let Some(hex) = character.to_digit(16) else {
                        self.invalid = true;
                        return;
                    };
                    let value = (value << 4) | hex as u16;
                    let digits = digits + 1;
                    if digits != 4 {
                        self.escape = Escape::Unicode { value, digits };
                        return;
                    }
                    self.escape = Escape::None;
                    if (0xd800..=0xdbff).contains(&value) && self.high_surrogate.is_none() {
                        self.high_surrogate = Some(value);
                        return;
                    }
                    let scalar = match self.high_surrogate.take() {
                        Some(high) if (0xdc00..=0xdfff).contains(&value) => {
                            0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(value) - 0xdc00
                        }
                        Some(_) => {
                            self.invalid = true;
                            return;
                        }
                        None => u32::from(value),
                    };
                    if let Some(decoded) = char::from_u32(scalar) {
                        self.push(role, decoded);
                    } else {
                        self.invalid = true;
                    }
                }
                Escape::None => match character {
                    '\\' => self.escape = Escape::Slash,
                    '"' if self.high_surrogate.is_none() => {
                        if matches!(role, StringRole::Key) {
                            self.key = std::mem::take(&mut self.key_buffer);
                            self.expecting_key = false;
                        }
                        self.role = None;
                    }
                    character if character >= '\u{0020}' => self.push(role, character),
                    _ => self.invalid = true,
                },
            }
            return;
        }
        match character {
            '{' | '[' => {
                self.depth += 1;
                if self.depth == 1 {
                    self.expecting_key = true;
                }
            }
            '}' | ']' => self.depth = self.depth.saturating_sub(1),
            ',' if self.depth == 1 => {
                self.expecting_key = true;
                self.key.clear();
            }
            '"' => {
                let role = if self.depth == 1 && self.expecting_key {
                    StringRole::Key
                } else if self.depth == 1 && self.key == "script" {
                    self.lines.clear();
                    self.visible_end = None;
                    self.revision += 1;
                    StringRole::Script
                } else {
                    StringRole::Other
                };
                self.role = Some(role);
            }
            _ => {}
        }
    }

    fn push(&mut self, role: StringRole, character: char) {
        if self.high_surrogate.is_some() {
            self.invalid = true;
            return;
        }
        match role {
            StringRole::Key => self.key_buffer.push(character),
            StringRole::Other => {}
            StringRole::Script => {
                if self.lines.is_empty() {
                    self.lines.push(String::new());
                }
                if character == '\n' {
                    if let Some(line) = self.lines.last_mut().filter(|line| line.ends_with('\r')) {
                        line.pop();
                    }
                    self.lines.push(String::new());
                } else {
                    let index = self.lines.len() - 1;
                    let line = &mut self.lines[index];
                    line.push(character);
                    if !character.is_whitespace() {
                        self.visible_end = Some((index, line.len()));
                    }
                }
                self.revision += 1;
            }
        }
    }
}

#[cfg(test)]
#[path = "interactive_presenter_script_stream_tests.rs"]
mod tests;
