use std::sync::Arc;

use super::{palette::PALETTE_CACHE_TTL, App, CommandChoice, CommandChoiceKind};
use crate::commands;

impl App {
    /// Command matches when the command palette is what the composer shows.
    ///
    /// Callers must already have excluded other composer modes; this owns the
    /// command-palette-specific guards.
    pub(super) fn visible_command_matches(&mut self) -> Option<Vec<CommandChoice>> {
        if self.input_ui.command_palette_dismissed() {
            return None;
        }
        let matches = self.command_matches();
        (!matches.is_empty()
            && (self.cursor_in_command_token()
                || !commands::argument_choices(self.input_ui.text(), self.input_ui.cursor())
                    .is_empty()
                || !self.mcp_argument_choices().is_empty()))
        .then_some(matches)
    }

    pub(super) fn command_matches(&mut self) -> Vec<CommandChoice> {
        let argument_choices =
            commands::argument_choices(self.input_ui.text(), self.input_ui.cursor());
        if !argument_choices.is_empty() {
            return argument_choices
                .iter()
                .map(argument_command_choice)
                .collect();
        }

        // Argument values belong to a command the user has already settled, so
        // they are offered where command matching has nothing left to say
        // rather than mixed into the command list.
        let mcp_argument_choices = self.mcp_argument_choices();
        if !mcp_argument_choices.is_empty() {
            return mcp_argument_choices;
        }

        let Some(prefix) = commands::command_prefix(self.input_ui.text()) else {
            return Vec::new();
        };
        let prefix = prefix
            .strip_prefix('/')
            .unwrap_or(prefix)
            .to_ascii_lowercase();
        let builtin_matches = commands::matching_commands(&prefix);
        let exact_builtin = builtin_matches
            .iter()
            .find(|command| command.name.eq_ignore_ascii_case(&prefix))
            .copied();
        let mut matches = builtin_matches
            .into_iter()
            .map(|command| CommandChoice {
                name: command.name.to_string(),
                usage: command.usage.to_string(),
                description: command.description.to_string(),
                kind: CommandChoiceKind::Builtin(command),
            })
            .collect::<Vec<_>>();
        if let Some(command) = exact_builtin {
            matches.extend(command.argument_choices.iter().map(argument_command_choice));
        }
        let mut template_matches = self
            .info
            .runtime
            .prompt_templates
            .iter()
            .filter(|(name, _)| crate::prompt_templates::matches_search(name, &prefix))
            .map(|(name, template)| {
                let command_name = format!("prompt:{name}");
                CommandChoice {
                    usage: format!("/{command_name} [text]"),
                    name: command_name,
                    description: crate::prompt_templates::description(template),
                    kind: CommandChoiceKind::PromptTemplate(template.clone()),
                }
            })
            .collect::<Vec<_>>();
        // prompt_templates is a BTreeMap, so iteration is already name-ordered.
        if let Some(index) = template_matches.iter().position(|choice| {
            choice
                .name
                .strip_prefix("prompt:")
                .is_some_and(|name| name.eq_ignore_ascii_case(&prefix))
        }) {
            let exact = template_matches.remove(index);
            matches.insert(0, exact);
        }
        matches.extend(template_matches);
        // The catalog is listed once at connect, so palette matching stays a
        // local lookup. Prompts are already ordered by server then name.
        matches.extend(
            self.mcp_catalog
                .prompts()
                .into_iter()
                .filter(|prompt| prompt.command_name().starts_with(&prefix))
                .map(|prompt| CommandChoice {
                    usage: prompt.usage(),
                    // A server that wrote no description still named the prompt,
                    // so fall back to that rather than to the same sentence for
                    // every prompt on the server.
                    description: prompt.description.clone().unwrap_or_else(|| {
                        format!("{} · from MCP server `{}`", prompt.label(), prompt.server)
                    }),
                    name: prompt.command_name(),
                    kind: CommandChoiceKind::McpPrompt,
                }),
        );
        // discovered skills are sorted by name; filtering preserves that order.
        matches.extend(
            self.discovered_skills()
                .iter()
                .filter(|skill| {
                    skill.name.starts_with(&prefix)
                        || format!("skill:{}", skill.name).starts_with(&prefix)
                })
                .map(|skill| {
                    let command_name = format!("skill:{}", skill.name);
                    CommandChoice {
                        usage: format!("/{command_name}"),
                        name: command_name,
                        description: skill.description.clone(),
                        kind: CommandChoiceKind::Skill,
                    }
                }),
        );
        matches
    }

    /// Skills for palette matching, served from the timed cache when fresh.
    ///
    /// Get-or-discover: whichever path asks first — a keystroke or a render
    /// frame — walks skill directories once and shares the result.
    fn discovered_skills(&mut self) -> Arc<Vec<crate::skills::Skill>> {
        if let Some(skills) = self.palette_caches.fresh_skills(PALETTE_CACHE_TTL) {
            return skills;
        }
        let skills = Arc::new(crate::skills::discover(&self.info.runtime.cwd));
        self.palette_caches.store_skills(Arc::clone(&skills));
        skills
    }

    #[cfg(test)]
    pub(super) fn selected_command(&mut self) -> Option<CommandChoice> {
        let matches = self.command_matches();
        selected_command(&matches, self.input_ui.command_selection())
    }

    pub(super) fn dismiss_command_palette_on_esc(&mut self) {
        if self.input_ui.text() == "/" {
            self.input_ui.clear_text();
            self.input_ui.set_cursor(0);
        }
        self.input_ui.set_command_palette_dismissed(true);
        self.input_ui.set_command_selection(0);
    }

    pub(super) fn complete_command_choice(&mut self, choice: &CommandChoice) {
        let (input, cursor) = match &choice.kind {
            CommandChoiceKind::Builtin(spec) => {
                self.input_ui
                    .set_submission_mode(super::InputSubmissionMode::ParseCommands);
                commands::complete_command(self.input_ui.text(), self.input_ui.cursor(), spec)
            }
            CommandChoiceKind::BuiltinArgument(choice) => {
                self.input_ui
                    .set_submission_mode(super::InputSubmissionMode::ParseCommands);
                commands::complete_argument_choice(choice)
            }
            CommandChoiceKind::PromptTemplate(template) => {
                let expanded_input = self.expanded_input();
                let mut input =
                    crate::prompt_templates::expand(template, slash_command_args(&expanded_input));
                input.push(' ');
                let cursor = input.chars().count();
                self.input_ui.clear_paste_segments();
                self.input_ui
                    .set_submission_mode(super::InputSubmissionMode::Prompt);
                (input, cursor)
            }
            // Both complete to a slash token and expand on submit: a skill
            // needs a tool call, and an MCP prompt needs a `prompts/get`
            // round-trip, neither of which can happen in this sync path.
            CommandChoiceKind::Skill | CommandChoiceKind::McpPrompt => {
                self.input_ui
                    .set_submission_mode(super::InputSubmissionMode::ParseCommands);
                complete_slash_command(self.input_ui.text(), self.input_ui.cursor(), &choice.name)
            }
            // Only the argument's own range is rewritten: the command and every
            // other argument already typed stay exactly as they are.
            CommandChoiceKind::McpPromptArgument { value } => {
                self.input_ui
                    .set_submission_mode(super::InputSubmissionMode::ParseCommands);
                super::mcp_argument_completion::replace_value(
                    self.input_ui.text(),
                    value,
                    &choice.name,
                )
            }
        };
        self.input_ui.set_text_and_cursor(input, cursor);
        self.input_ui.set_shell_mode(None);
    }
}

/// The palette row currently selected among already-resolved matches.
pub(super) fn selected_command(
    matches: &[CommandChoice],
    selection: usize,
) -> Option<CommandChoice> {
    matches
        .get(selection.min(matches.len().saturating_sub(1)))
        .cloned()
}

fn argument_command_choice(choice: &'static commands::CommandArgumentChoice) -> CommandChoice {
    CommandChoice {
        name: choice.completion.to_string(),
        usage: choice.usage.to_string(),
        description: choice.description.to_string(),
        kind: CommandChoiceKind::BuiltinArgument(choice),
    }
}

pub(super) fn slash_command_args(input: &str) -> &str {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    input[token_end..].trim_start()
}

pub(super) fn complete_slash_command(input: &str, cursor: usize, name: &str) -> (String, usize) {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    let token_len = input[..token_end].chars().count();
    let args = slash_command_args(input);
    let completed = if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    };
    let completed_token_len = name.chars().count() + 1;
    let new_cursor = if cursor <= token_len {
        completed_token_len
    } else {
        completed
            .chars()
            .count()
            .min(completed_token_len.saturating_add(cursor.saturating_sub(token_len)))
    };
    (completed, new_cursor)
}

#[cfg(test)]
#[path = "command_palette_tests.rs"]
mod tests;
