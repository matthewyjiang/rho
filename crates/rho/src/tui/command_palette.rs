use std::sync::Arc;

use super::{App, CommandChoice, CommandChoiceKind, SkillMatchCache};
use crate::commands;

/// How long one skill-discovery pass stays valid for command palette queries.
const SKILL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

impl App {
    pub(super) fn command_matches(&self) -> Vec<CommandChoice> {
        let argument_choices = commands::argument_choices(&self.input, self.input_cursor);
        if !argument_choices.is_empty() {
            return argument_choices
                .iter()
                .map(argument_command_choice)
                .collect();
        }

        let Some(prefix) = commands::command_prefix(&self.input) else {
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
        template_matches.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        });
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
        let mut skill_matches = self
            .discovered_skills()
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
            })
            .collect::<Vec<_>>();
        skill_matches.sort_by(|left, right| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        });
        matches.extend(skill_matches);
        matches
    }

    /// Skills for palette matching, served from the timed cache when fresh so
    /// repeated per-keystroke queries skip the filesystem walk.
    fn discovered_skills(&self) -> Arc<Vec<crate::skills::Skill>> {
        if let Some(cache) = &self.skill_match_cache {
            if cache.refreshed_at.elapsed() < SKILL_CACHE_TTL {
                return Arc::clone(&cache.skills);
            }
        }
        Arc::new(crate::skills::discover(&self.info.runtime.cwd))
    }

    pub(super) fn refresh_skill_match_cache(&mut self) {
        if self
            .skill_match_cache
            .as_ref()
            .is_some_and(|cache| cache.refreshed_at.elapsed() < SKILL_CACHE_TTL)
        {
            return;
        }
        self.skill_match_cache = Some(SkillMatchCache {
            skills: Arc::new(crate::skills::discover(&self.info.runtime.cwd)),
            refreshed_at: std::time::Instant::now(),
        });
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
            CommandChoiceKind::BuiltinArgument(choice) => {
                self.input_submission_mode = super::InputSubmissionMode::ParseCommands;
                commands::complete_argument_choice(choice)
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

fn argument_command_choice(choice: &'static commands::CommandArgumentChoice) -> CommandChoice {
    CommandChoice {
        name: choice.completion.to_string(),
        usage: choice.usage.to_string(),
        description: choice.description.to_string(),
        kind: CommandChoiceKind::BuiltinArgument(choice),
    }
}

#[cfg(test)]
#[path = "command_palette_tests.rs"]
mod tests;
