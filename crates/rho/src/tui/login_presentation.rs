//! Composer presentation for a pending interactive login.
//!
//! First-run setup paints the composer, not the transcript, so the authorize
//! URL and code live here.

use std::ops::Range;

use ratatui::{
    layout::{Position, Rect},
    text::{Line, Span},
};
use rho_providers::auth::login_prompt::LoginPrompt;

use super::{
    markdown::{code_block_copy_columns, code_block_copy_label},
    render::{display_width, styled_line, truncate_one_line, truncate_to_display_width, LineFill},
    theme::Theme,
    PendingLoginComposer,
};

pub(super) const LOGIN_KEY_HINT: &str = "c copy  Esc cancel";

/// Screen-relative copy target produced with the composer lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CopyHit {
    /// Line index in the composer lines, before any visible-start skip.
    pub row: usize,
    pub columns: Range<usize>,
    pub text: String,
}

impl CopyHit {
    /// Hit-test a pointer against this target as painted at `origin`, skipping
    /// `start` composer lines the same way the renderer does.
    pub(super) fn text_at(
        &self,
        origin: Rect,
        start: usize,
        column: u16,
        row: u16,
    ) -> Option<&str> {
        if !origin.contains(Position { x: column, y: row }) {
            return None;
        }
        let line = start.saturating_add(row.saturating_sub(origin.y) as usize);
        if line != self.row {
            return None;
        }
        let rel_col = column.saturating_sub(origin.x) as usize;
        self.columns
            .contains(&rel_col)
            .then_some(self.text.as_str())
    }
}

pub(super) struct LoginComposerView {
    pub lines: Vec<Line<'static>>,
    pub copy_hit: Option<CopyHit>,
}

pub(super) fn login_composer_view(
    pending: &PendingLoginComposer,
    width: usize,
) -> LoginComposerView {
    prompt_composer_view(&pending.target.label, &pending.prompt, width)
}

fn prompt_composer_view(label: &str, prompt: &LoginPrompt, width: usize) -> LoginComposerView {
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
    lines.push(url_line(&prompt.url, width));
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
    let copy_hit = code_block_copy_columns(width).map(|columns| CopyHit {
        row: copy_row,
        columns,
        text: prompt.copyable_url().to_string(),
    });
    LoginComposerView { lines, copy_hit }
}

fn url_line(url: &str, width: usize) -> Line<'static> {
    let copy_columns = code_block_copy_columns(width);
    let copy_label = copy_columns
        .as_ref()
        .and_then(|_| code_block_copy_label(width));
    let label_budget = copy_columns
        .as_ref()
        .map_or(width, |columns| columns.start.saturating_sub(1));
    let label = truncate_to_display_width(url, label_budget);
    let mut spans = Vec::new();
    if let Some(columns) = &copy_columns {
        let filler = columns.start.saturating_sub(display_width(&label));
        spans.push(Span::styled(
            format!("{label}{}", " ".repeat(filler)),
            Theme::accent(),
        ));
    } else {
        spans.push(Span::styled(label.into_owned(), Theme::accent()));
    }
    if let Some(copy_label) = copy_label {
        spans.push(Span::styled(
            copy_label,
            Theme::markdown_code_copy_button(/*hovered*/ false),
        ));
    }
    Line::from(spans)
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
