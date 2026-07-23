//! Composer chrome rendering: input lines, cursor, divider, and palette suggestions.

use ratatui::{layout::Position, text::Line};

use super::{
    approval_lines, char_prefix_display_width, config_number_input_lines, config_text_input_lines,
    display_width, file_picker,
    inline_choice::inline_choice_lines,
    inline_shell, input_cursor_position, input_lines_with_images, labeled_divider_line,
    login::{oauth_pending_lines, secret_input_lines},
    picker_lines, questionnaire_cursor_position, questionnaire_lines, styled_line,
    truncate_one_line, App, ComposerMode, LineFill, Theme, MAX_COMMAND_SUGGESTIONS,
    MIN_COMMAND_DESCRIPTION_WIDTH,
};

impl App {
    pub(super) fn divider_line(&self, width: usize, shell_label: bool) -> Line<'static> {
        let width = width.max(1);
        let (style, labels) = match &self.input_ui.composer {
            ComposerMode::Input => {
                let labels = shell_label
                    .then_some(self.input_ui.shell_mode)
                    .flatten()
                    .map(inline_shell::mode_divider_labels);
                (
                    Theme::reasoning_input_border(self.info.runtime.reasoning),
                    labels,
                )
            }
            ComposerMode::Picker(_)
            | ComposerMode::Questionnaire(_)
            | ComposerMode::Approval(_)
            | ComposerMode::InlineChoice(_) => (Theme::input_prompt(), None),
            ComposerMode::SecretInput(_)
            | ComposerMode::ConfigNumberInput(_)
            | ComposerMode::ConfigTextInput(_)
            | ComposerMode::OAuthPending(_) => (Theme::dim(), None),
        };
        if let Some(labels) = labels {
            if let Some(line) = labeled_divider_line(labels, style, width) {
                return line;
            }
        }
        Line::styled("─".repeat(width), style)
    }

    pub(super) fn composer_lines(&self, width: usize) -> Vec<Line<'static>> {
        match &self.input_ui.composer {
            ComposerMode::Input => {
                let focused_paste = self
                    .focused_paste_segment()
                    .map(|segment| segment.start..segment.end());
                input_lines_with_images(
                    &self.input_ui.text,
                    &self.input_ui.pending_images,
                    width,
                    focused_paste,
                )
            }
            ComposerMode::Picker(picker) if picker.is_overlay() => Vec::new(),
            ComposerMode::Picker(picker) => picker_lines(picker, width),
            ComposerMode::SecretInput(secret) => secret_input_lines(secret, width),
            ComposerMode::ConfigNumberInput(input) => config_number_input_lines(input, width),
            ComposerMode::ConfigTextInput(input) => config_text_input_lines(input, width),
            ComposerMode::OAuthPending(target) => oauth_pending_lines(target, width),
            ComposerMode::InlineChoice(modal) => inline_choice_lines(&modal.choice, width),
            ComposerMode::Questionnaire(questionnaire) => questionnaire_lines(questionnaire, width),
            ComposerMode::Approval(approval) => approval_lines(approval, width),
        }
    }

    pub(super) fn composer_cursor_position(&self, width: usize) -> Position {
        match &self.input_ui.composer {
            ComposerMode::Input => {
                let mut position =
                    input_cursor_position(&self.input_ui.text, self.input_ui.cursor, width);
                position.y = position
                    .y
                    .saturating_add(self.input_ui.pending_images.len() as u16);
                position
            }
            ComposerMode::SecretInput(secret) => Position {
                x: char_prefix_display_width(&secret.value, secret.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigNumberInput(input) => Position {
                x: char_prefix_display_width(&input.value, input.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::ConfigTextInput(input) => Position {
                x: char_prefix_display_width(&input.value, input.cursor).min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire_cursor_position(questionnaire, width)
            }
            ComposerMode::OAuthPending(_)
            | ComposerMode::Approval(_)
            | ComposerMode::InlineChoice(_) => Position { x: 0, y: 0 },
            ComposerMode::Picker(picker) => Position {
                x: display_width(&picker.filter)
                    .saturating_add(2)
                    .min(width.saturating_sub(1)) as u16,
                y: 0,
            },
        }
    }

    pub(super) fn command_suggestion_lines(&self, width: usize) -> Vec<Line<'static>> {
        if self.command_palette_visible() {
            let matches = self.command_matches();
            let selected_index = self
                .input_ui
                .command_selection
                .min(matches.len().saturating_sub(1));
            let start = selected_index
                .saturating_add(1)
                .saturating_sub(MAX_COMMAND_SUGGESTIONS);

            let usage_width = matches
                .iter()
                .skip(start)
                .take(MAX_COMMAND_SUGGESTIONS)
                .map(|command| display_width(&command.usage))
                .max()
                .unwrap_or(1)
                .min(
                    width
                        .saturating_sub(MIN_COMMAND_DESCRIPTION_WIDTH + 3)
                        .max(1),
                );

            return matches
                .into_iter()
                .enumerate()
                .skip(start)
                .take(MAX_COMMAND_SUGGESTIONS)
                .map(|(index, command)| {
                    let selected = index == selected_index;
                    let marker = if selected { ">" } else { " " };
                    let description_width = width.saturating_sub(usage_width + 3).max(1);
                    let usage = truncate_one_line(&command.usage, usage_width);
                    let description = truncate_one_line(&command.description, description_width);
                    let usage_padding =
                        " ".repeat(usage_width.saturating_sub(display_width(&usage)));
                    let text = format!("{marker} {usage}{usage_padding} {description}");
                    let style = if selected {
                        Theme::brand()
                    } else {
                        Theme::dim()
                    };
                    styled_line(text, width.max(1), style, LineFill::Natural)
                })
                .collect();
        }

        if !self.file_palette_visible() {
            return Vec::new();
        }

        let matches = self.file_matches();
        let selected_index = self
            .input_ui
            .file_selection
            .min(matches.len().saturating_sub(1));
        let (start, above, below) = file_picker::file_palette_scroll_counts(
            matches.len(),
            selected_index,
            MAX_COMMAND_SUGGESTIONS,
        );

        let mut lines = matches
            .iter()
            .enumerate()
            .skip(start)
            .take(MAX_COMMAND_SUGGESTIONS)
            .map(|(index, path)| {
                let selected = index == selected_index;
                let marker = if selected { ">" } else { " " };
                let text = format!("{marker} @{path}");
                let style = if selected {
                    Theme::brand()
                } else {
                    Theme::dim()
                };
                styled_line(
                    truncate_one_line(&text, width.max(1)),
                    width.max(1),
                    style,
                    LineFill::Natural,
                )
            })
            .collect::<Vec<_>>();

        if let Some(footer) = file_picker::file_palette_scroll_footer(above, below, matches.len()) {
            lines.push(styled_line(
                truncate_one_line(&footer, width.max(1)),
                width.max(1),
                Theme::dim(),
                LineFill::Natural,
            ));
        }

        lines
    }
}
