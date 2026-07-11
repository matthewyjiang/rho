use super::{App, CommandChoice, CommandChoiceKind};
use crate::commands;

impl App {
    pub(super) fn command_matches(&self) -> Vec<CommandChoice> {
        let Some(prefix) = commands::command_prefix(&self.input) else {
            return Vec::new();
        };
        let prefix = prefix
            .strip_prefix('/')
            .unwrap_or(prefix)
            .to_ascii_lowercase();
        let mut matches = commands::matching_commands(&prefix)
            .into_iter()
            .map(|command| CommandChoice {
                name: command.name.to_string(),
                usage: command.usage.to_string(),
                description: command.description.to_string(),
                kind: CommandChoiceKind::Builtin(command),
            })
            .collect::<Vec<_>>();
        let mut template_matches = self
            .info
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
        matches.extend(
            crate::skills::discover(&self.info.cwd)
                .into_iter()
                .filter(|skill| {
                    skill.name.starts_with(&prefix)
                        || format!("skill:{}", skill.name).starts_with(&prefix)
                })
                .map(|skill| {
                    let command_name = format!("skill:{}", skill.name);
                    CommandChoice {
                        usage: format!("/{command_name}"),
                        name: command_name,
                        description: skill.description,
                        kind: CommandChoiceKind::Skill,
                    }
                }),
        );
        matches
    }

    pub(super) fn selected_command(&self) -> Option<CommandChoice> {
        let matches = self.command_matches();
        matches
            .get(self.command_selection.min(matches.len().saturating_sub(1)))
            .cloned()
    }

    pub(super) fn complete_command_choice(&mut self, choice: &CommandChoice) {
        let (input, cursor) = match &choice.kind {
            CommandChoiceKind::Builtin(spec) => {
                self.input_submission_mode = super::InputSubmissionMode::ParseCommands;
                commands::complete_command(&self.input, self.input_cursor, spec)
            }
            CommandChoiceKind::PromptTemplate(template) => {
                let expanded_input = self.expanded_input();
                let mut input = crate::prompt_templates::expand(
                    template,
                    super::slash_command_args(&expanded_input),
                );
                input.push(' ');
                let cursor = input.chars().count();
                self.paste_segments.clear();
                self.input_submission_mode = super::InputSubmissionMode::Prompt;
                (input, cursor)
            }
            CommandChoiceKind::Skill => {
                self.input_submission_mode = super::InputSubmissionMode::ParseCommands;
                super::complete_slash_command(&self.input, self.input_cursor, &choice.name)
            }
        };
        self.input = input;
        self.input_cursor = cursor;
    }
}

#[cfg(test)]
#[path = "command_palette_tests.rs"]
mod tests;
