//! Secret API-key collection for interactive provider login.

use rho_providers::model::catalog::LoginTarget;

use super::{composer_chrome, styled_line, truncate_one_line, LineFill, Theme};

#[derive(Clone, Debug)]
pub(super) struct SecretInput {
    pub(super) target: LoginTarget,
    pub(super) value: String,
    pub(super) cursor: usize,
}

impl SecretInput {
    pub(super) fn new(target: LoginTarget) -> Self {
        Self {
            target,
            value: String::new(),
            cursor: 0,
        }
    }

    pub(super) fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        let sanitized = text.replace('\n', "");
        let byte_index = self.byte_index(self.cursor);
        self.value.insert_str(byte_index, &sanitized);
        self.cursor += sanitized.chars().count();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }
}

pub(super) fn secret_input_lines(
    secret: &SecretInput,
    width: usize,
) -> Vec<ratatui::text::Line<'static>> {
    let prompt = format!(
        "enter {}  {}",
        secret.target.label,
        composer_chrome::join_footer_parts(["Enter save", "Esc cancel"])
    );
    let display_value = "•".repeat(secret.value.chars().count());
    vec![
        styled_line(
            truncate_one_line(&prompt, width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&display_value, width),
            width,
            Theme::text(),
            LineFill::Natural,
        ),
    ]
}
