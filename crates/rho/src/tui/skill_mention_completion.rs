//! `$skill` completion in the composer.
//!
//! A whitespace-delimited token that starts with `$` asks the command palette
//! for skills. Picking one rewrites only that token, so the rest of the prompt
//! stays as typed. Submitting loads every mentioned skill; see
//! [`crate::skills::mention`].

use std::ops::Range;

use super::{App, CommandChoice, CommandChoiceKind, ComposerMode};

/// The `$` token under the cursor: its char range, sigil included, and the
/// name typed before the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SkillMentionCursor {
    pub(super) token: Range<usize>,
    pub(super) typed: String,
}

impl SkillMentionCursor {
    /// What the palette is answering, so a changed query resets the
    /// selection.
    pub(super) fn palette_identity(&self) -> String {
        format!("${}", self.typed)
    }
}

impl App {
    /// The mention the composer cursor sits in, when a palette could show
    /// for it.
    pub(super) fn skill_mention_cursor(&self) -> Option<SkillMentionCursor> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input)
            || self.input_ui.shell_mode().is_some()
            || self.input_ui.command_palette_dismissed()
        {
            return None;
        }
        mention_under_cursor(self.input_ui.text(), self.input_ui.cursor())
    }

    /// Palette rows for the skills whose names start with the typed name.
    pub(super) fn skill_mention_choices(
        &mut self,
        cursor: &SkillMentionCursor,
    ) -> Vec<CommandChoice> {
        self.discovered_skills()
            .iter()
            .filter(|skill| {
                skill.name.starts_with(&cursor.typed)
                    && crate::skills::mention::mentionable(&skill.name)
            })
            .map(|skill| CommandChoice {
                name: skill.name.clone(),
                usage: format!("${}", skill.name),
                description: skill.description.clone(),
                kind: CommandChoiceKind::SkillMention {
                    token: cursor.token.clone(),
                },
            })
            .collect()
    }
}

/// The `$` token holding `cursor`, a char index. The token is the run of
/// non-whitespace around the cursor; it must start with `$` before the cursor
/// and hold only skill-name characters after it.
pub(super) fn mention_under_cursor(text: &str, cursor: usize) -> Option<SkillMentionCursor> {
    let chars = text.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|ch| ch.is_whitespace())
        .map_or(0, |index| index + 1);
    let end = chars[cursor..]
        .iter()
        .position(|ch| ch.is_whitespace())
        .map_or(chars.len(), |offset| cursor + offset);
    if start == cursor || chars[start] != '$' {
        return None;
    }
    let name = &chars[start + 1..end];
    if !name
        .iter()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':'))
    {
        return None;
    }
    Some(SkillMentionCursor {
        token: start..end,
        typed: chars[start + 1..cursor].iter().collect(),
    })
}

/// `text` with the `token` char range replaced by `$name`, and the cursor
/// after the space that ends the mention.
pub(super) fn complete_mention(text: &str, token: &Range<usize>, name: &str) -> (String, usize) {
    let chars = text.chars().collect::<Vec<_>>();
    let before = chars[..token.start].iter().collect::<String>();
    let after = chars[token.end..].iter().collect::<String>();
    let separator = if after.starts_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let completed = format!("{before}${name}{separator}{after}");
    let cursor = token.start + 1 + name.chars().count() + 1;
    (completed, cursor)
}

#[cfg(test)]
#[path = "skill_mention_completion_tests.rs"]
mod tests;
