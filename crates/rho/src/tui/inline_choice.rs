use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use super::{styled_line, truncate_one_line, LineFill, Theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InlineChoiceOption {
    pub(super) value: String,
    pub(super) shortcut: char,
    alternate_shortcut: Option<char>,
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) available: bool,
}

impl InlineChoiceOption {
    pub(super) fn available(
        value: impl Into<String>,
        shortcut: char,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            value: value.into(),
            shortcut,
            alternate_shortcut: None,
            label: label.into(),
            detail: detail.into(),
            available: true,
        }
    }

    pub(super) fn with_alternate_shortcut(mut self, shortcut: char) -> Self {
        self.alternate_shortcut = Some(shortcut);
        self
    }

    pub(super) fn unavailable(
        value: impl Into<String>,
        shortcut: char,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            available: false,
            ..Self::available(value, shortcut, label, detail)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InlineChoice {
    pub(super) title: String,
    pub(super) description: String,
    pub(super) options: Vec<InlineChoiceOption>,
    active: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InlineChoiceKeyOutcome {
    Handled,
    Cancelled,
    Selected(String),
}

impl InlineChoice {
    pub(super) fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        options: Vec<InlineChoiceOption>,
    ) -> anyhow::Result<Self> {
        let active = options
            .iter()
            .position(|option| option.available)
            .ok_or_else(|| anyhow::anyhow!("inline choice has no available options"))?;
        Ok(Self {
            title: title.into(),
            description: description.into(),
            options,
            active,
        })
    }

    pub(super) fn selected_value(&self) -> &str {
        &self.options[self.active].value
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> InlineChoiceKeyOutcome {
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter | KeyCode::Char(' ')) => {
                InlineChoiceKeyOutcome::Selected(self.selected_value().to_string())
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(key)) => {
                let normalized = key.to_ascii_lowercase();
                let Some(index) = self.options.iter().position(|option| {
                    option.available
                        && (option.shortcut.eq_ignore_ascii_case(&normalized)
                            || option
                                .alternate_shortcut
                                .is_some_and(|shortcut| shortcut.eq_ignore_ascii_case(&normalized)))
                }) else {
                    return InlineChoiceKeyOutcome::Handled;
                };
                self.active = index;
                InlineChoiceKeyOutcome::Selected(self.selected_value().to_string())
            }
            (_, KeyCode::Esc) => InlineChoiceKeyOutcome::Cancelled,
            (_, KeyCode::Up | KeyCode::Left) => {
                self.move_previous();
                InlineChoiceKeyOutcome::Handled
            }
            (_, KeyCode::Down | KeyCode::Right) => {
                self.move_next();
                InlineChoiceKeyOutcome::Handled
            }
            _ => InlineChoiceKeyOutcome::Handled,
        }
    }

    fn move_previous(&mut self) {
        if let Some(index) = (0..self.active)
            .rev()
            .find(|index| self.options[*index].available)
        {
            self.active = index;
        }
    }

    fn move_next(&mut self) {
        if let Some(index) =
            (self.active + 1..self.options.len()).find(|index| self.options[*index].available)
        {
            self.active = index;
        }
    }
}

pub(super) fn inline_choice_lines(choice: &InlineChoice, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = vec![
        styled_line(
            truncate_one_line(&choice.title, width),
            width,
            Theme::input_prompt(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(&choice.description, width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
    ];

    for (index, option) in choice.options.iter().enumerate() {
        let selected = index == choice.active && option.available;
        let marker = if selected {
            ">"
        } else if option.available {
            " "
        } else {
            "·"
        };
        let style = if selected {
            Theme::input_prompt()
        } else if option.available {
            Theme::text()
        } else {
            Theme::dim()
        };
        lines.push(styled_line(
            truncate_one_line(
                &format!("{marker} [{}] {}", option.shortcut, option.label),
                width,
            ),
            width,
            style,
            LineFill::Natural,
        ));
        lines.push(styled_line(
            truncate_one_line(&format!("      {}", option.detail), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
    }

    lines.push(styled_line(
        truncate_one_line(
            "enter/space choose · shortcut choose · arrows move · esc cancel",
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines
}

#[cfg(test)]
#[path = "inline_choice_tests.rs"]
mod tests;
