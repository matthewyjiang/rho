//! Composer presentation for a pending interactive login.
//!
//! First-run setup paints the composer, not the transcript, so the authorize
//! URL and code live here.

use ratatui::text::Line;
use rho_providers::auth::login_prompt::LoginPrompt;

use super::{
    copy_interaction::CopyHit,
    markdown::copyable_header_line,
    render::{styled_line, truncate_one_line, LineFill},
    theme::Theme,
    PendingLoginComposer,
};

pub(super) const LOGIN_KEY_HINT: &str = "c copy  Esc cancel";

pub(super) struct LoginComposerView {
    pub lines: Vec<Line<'static>>,
    pub copy_hit: Option<CopyHit>,
}

pub(super) fn login_composer_view(
    pending: &PendingLoginComposer,
    width: usize,
    hovered: bool,
) -> LoginComposerView {
    prompt_composer_view(&pending.target.label, &pending.prompt, width, hovered)
}

fn prompt_composer_view(
    label: &str,
    prompt: &LoginPrompt,
    width: usize,
    hovered: bool,
) -> LoginComposerView {
    let mut lines = vec![styled_line(
        truncate_one_line(&format!("waiting for {label} login"), width),
        width,
        Theme::dim(),
        LineFill::Natural,
    )];
    lines.push(styled_line(
        truncate_one_line(&prompt.instruction, width),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    let copy_row = lines.len();
    let (url_line, copy_columns) =
        copyable_header_line(&prompt.url, width, Theme::accent(), Some(hovered));
    lines.push(url_line);
    if let Some(code) = &prompt.user_code {
        lines.push(styled_line(
            truncate_one_line(&format!("code {code}"), width),
            width,
            Theme::text_strong(),
            LineFill::Natural,
        ));
    }
    lines.push(styled_line(
        truncate_one_line(prompt.browser.note(), width),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines.push(styled_line(
        truncate_one_line(LOGIN_KEY_HINT, width),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    let copy_hit = copy_columns.map(|columns| CopyHit {
        row: copy_row,
        columns,
        text: prompt.copyable_url().to_string(),
    });
    LoginComposerView { lines, copy_hit }
}

pub(super) fn notice_lines(provider_label: &str, prompt: &LoginPrompt) -> Vec<String> {
    vec![
        prompt.url.clone(),
        format!("{provider_label} login pending"),
    ]
}

#[cfg(test)]
#[path = "login_presentation_tests.rs"]
mod tests;
