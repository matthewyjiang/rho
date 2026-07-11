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
        matches.extend(
            self.info
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
                }),
        );
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

    pub(super) fn complete_command_choice(&self, choice: &CommandChoice) -> (String, usize) {
        match &choice.kind {
            CommandChoiceKind::Builtin(spec) => {
                commands::complete_command(&self.input, self.input_cursor, spec)
            }
            CommandChoiceKind::PromptTemplate(template) => {
                let mut input = crate::prompt_templates::expand(
                    template,
                    super::slash_command_args(&self.input),
                );
                input.push(' ');
                let cursor = input.chars().count();
                (input, cursor)
            }
            CommandChoiceKind::Skill => {
                super::complete_slash_command(&self.input, self.input_cursor, &choice.name)
            }
        }
    }
}
