//! `$skill` commands for ACP hosts.
//!
//! T3 Code fills its `$` menu only from `available_commands_update` entries
//! named `$<skill>` and sends a picked skill back as a `$<skill>` mention in
//! plain prompt text. Rho advertises its discovered skills that way; the
//! shared resolver ([`crate::app::session_commands`]) then loads every
//! mentioned skill before the first model request, as one batch of
//! host-requested `skill` calls. [`crate::skills::mention`] owns the mention
//! rule shared with the TUI.

use agent_client_protocol::schema::v1::AvailableCommand;

use crate::skills::{mention, Skill};

/// `$<name>` commands for every skill a T3 composer can mention.
pub(super) fn available_commands(skills: &[Skill]) -> Vec<AvailableCommand> {
    skills
        .iter()
        .filter(|skill| mention::mentionable(&skill.name))
        .map(|skill| AvailableCommand::new(format!("${}", skill.name), skill.description.clone()))
        .collect()
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
