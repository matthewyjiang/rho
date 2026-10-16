//! `$skill` commands for ACP hosts.
//!
//! T3 Code fills its `$` menu only from `available_commands_update` entries
//! named `$<skill>` and sends a picked skill back as a `$<skill>` mention in
//! plain prompt text. Rho advertises its discovered skills that way, then
//! loads every mentioned skill before the first model request, as one batch
//! of host-requested `skill` calls. [`crate::skills::mention`] owns the
//! mention rule shared with the TUI; the command catalog in
//! [`super::commands`] owns when it applies.

use agent_client_protocol::schema::v1::AvailableCommand;
use rho_sdk::{
    model::{ContentBlock, ToolCall},
    UserInput,
};

pub(super) use crate::skills::mention::{load_call, SKILL_TOOL};
use crate::skills::{mention, Skill};

/// `$<name>` commands for every skill a T3 composer can mention.
pub(super) fn available_commands(skills: &[Skill]) -> Vec<AvailableCommand> {
    skills
        .iter()
        .filter(|skill| mention::mentionable(&skill.name))
        .map(|skill| AvailableCommand::new(format!("${}", skill.name), skill.description.clone()))
        .collect()
}

/// Host calls that load the `leading` skills, then every known skill the
/// prompt's text blocks mention, each skill once and in that order.
pub(super) fn load_calls(
    leading: &[&str],
    input: &UserInput,
    discover: impl FnOnce() -> Vec<Skill>,
) -> Vec<ToolCall> {
    let texts = input
        .blocks()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>();
    mention::skills_to_load(leading, &texts, discover)
        .into_iter()
        .map(load_call)
        .collect()
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
