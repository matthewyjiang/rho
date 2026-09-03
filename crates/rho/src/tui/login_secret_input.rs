//! Secret API-key collection for interactive provider login.

use rho_providers::model::catalog::LoginTarget;

use super::{
    composer_chrome, line_editor::LineEditor, styled_line, truncate_one_line, LineFill, Theme,
};

#[derive(Clone, Debug)]
pub(super) struct SecretInput {
    pub(super) target: LoginTarget,
    pub(super) editor: LineEditor,
    pub(super) allow_empty: bool,
}

impl SecretInput {
    pub(super) fn new(target: LoginTarget) -> Self {
        Self {
            target,
            editor: LineEditor::new(""),
            allow_empty: false,
        }
    }

    /// For hosts that also run keyless, where blank means "do not set a new key".
    pub(super) fn optional(target: LoginTarget) -> Self {
        Self {
            allow_empty: true,
            ..Self::new(target)
        }
    }

    /// What pressing Enter with the current value means.
    pub(super) fn submission(&self) -> super::login::ApiKeySubmission {
        let key = self.editor.value.trim().to_string();
        let target = self.target.clone();
        match (key.is_empty(), self.allow_empty) {
            (true, true) => super::login::ApiKeySubmission::LeaveUnset { target },
            (true, false) => super::login::ApiKeySubmission::Rejected,
            (false, _) => super::login::ApiKeySubmission::Save { target, key },
        }
    }
}

pub(super) fn secret_input_lines(
    secret: &SecretInput,
    width: usize,
) -> Vec<ratatui::text::Line<'static>> {
    let prompt = if secret.allow_empty {
        format!(
            "enter API key (optional)  {}",
            composer_chrome::join_footer_parts(["Enter save", "Esc cancel"])
        )
    } else {
        format!(
            "enter {}  {}",
            secret.target.label,
            composer_chrome::join_footer_parts(["Enter save", "Esc cancel"])
        )
    };
    let display_value = "•".repeat(secret.editor.value.chars().count());
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
