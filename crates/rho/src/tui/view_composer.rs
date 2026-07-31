//! Composer chrome rendering: input lines, cursor, divider, and palette suggestions.

use ratatui::{
    layout::Position,
    text::{Line, Span},
};

use super::{
    approval_lines, char_prefix_display_width,
    composer_layout::{content_width, prompt_width, PROMPT_PREFIX},
    config_number_input_lines, display_width, file_picker,
    inline_choice::inline_choice_lines,
    inline_shell, input_cursor_position, input_label_lines, input_lines, labeled_divider_line,
    login::{interactive_pending_lines, secret_input_lines},
    picker_lines, questionnaire_cursor_position, questionnaire_lines, styled_line,
    text_input::text_input_lines,
    truncate_one_line, App, ComposerMode, LineFill, Theme, MAX_COMMAND_SUGGESTIONS,
    MIN_COMMAND_DESCRIPTION_WIDTH,
};

impl App {
    pub(super) fn divider_line(&self, width: usize, shell_label: bool) -> Line<'static> {
        let width = width.max(1);
        let (style, labels) = match self.input_ui.composer() {
            ComposerMode::Input => {
                let labels = shell_label
                    .then_some(self.input_ui.shell_mode())
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
            | ComposerMode::TextInput(_)
            | ComposerMode::InteractivePending(_) => (Theme::dim(), None),
        };
        if let Some(labels) = labels {
            if let Some(line) = labeled_divider_line(labels, style, width) {
                return line;
            }
        }
        Line::styled("─".repeat(width), style)
    }

    /// Composer rows for a `width` by `viewport_height` screen.
    ///
    /// Pickers size their list to the viewport, so the height must be the real
    /// terminal height rather than a fallback.
    pub(super) fn composer_lines(
        &self,
        width: usize,
        viewport_height: usize,
    ) -> Vec<Line<'static>> {
        match self.input_ui.composer() {
            ComposerMode::Input => {
                let focused_paste = self
                    .focused_paste_segment()
                    .map(|segment| segment.start..segment.end());
                let media_labels = self
                    .input_ui
                    .attachments()
                    .iter()
                    .enumerate()
                    .map(|(index, media)| media.composer_label(index + 1))
                    .collect::<Vec<_>>();
                let mut lines = input_label_lines(&media_labels, width);
                let mut text_lines =
                    input_lines(self.input_ui.text(), content_width(width), focused_paste);
                for (index, line) in text_lines.iter_mut().enumerate() {
                    line.spans.insert(
                        0,
                        if index == 0 {
                            Span::styled(PROMPT_PREFIX, Theme::dim())
                        } else {
                            Span::raw(" ".repeat(prompt_width()))
                        },
                    );
                }
                if self.input_ui.text().is_empty() && self.input_ui.shell_mode().is_none() {
                    text_lines[0]
                        .spans
                        .push(Span::styled("Type a message", Theme::dim()));
                }
                lines.extend(text_lines);
                lines
            }
            ComposerMode::Picker(picker) if picker.is_overlay() => Vec::new(),
            ComposerMode::Picker(picker) => picker_lines(picker, width, viewport_height),
            ComposerMode::SecretInput(secret) => secret_input_lines(secret, width),
            ComposerMode::ConfigNumberInput(input) => config_number_input_lines(input, width),
            ComposerMode::TextInput(input) => text_input_lines(input, width),
            ComposerMode::InteractivePending(target) => interactive_pending_lines(target, width),
            ComposerMode::InlineChoice(modal) => inline_choice_lines(&modal.choice, width),
            ComposerMode::Questionnaire(questionnaire) => questionnaire_lines(questionnaire, width),
            ComposerMode::Approval(approval) => approval_lines(approval, width, viewport_height),
        }
    }

    pub(super) fn composer_cursor_position(&self, width: usize) -> Position {
        match self.input_ui.composer() {
            ComposerMode::Input => {
                let mut position = input_cursor_position(
                    self.input_ui.text(),
                    self.input_ui.cursor(),
                    content_width(width),
                );
                position.x = position
                    .x
                    .saturating_add(prompt_width() as u16)
                    .min(width.saturating_sub(1) as u16);
                position.y = position
                    .y
                    .saturating_add(self.input_ui.attachments().len() as u16);
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
            ComposerMode::TextInput(input) => Position {
                x: char_prefix_display_width(&input.editor.value, input.editor.cursor)
                    .min(width.max(1)) as u16,
                y: 1,
            },
            ComposerMode::Questionnaire(questionnaire) => {
                questionnaire_cursor_position(questionnaire, width)
            }
            ComposerMode::InteractivePending(_)
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
                .command_selection()
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
            .file_selection()
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

        if let Some(footer) = file_picker::file_palette_scroll_footer(
            above,
            below,
            matches.len(),
            self.file_discovery_incomplete(),
        ) {
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
