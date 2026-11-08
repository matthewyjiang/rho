//! The commands `rho acp` advertises and runs.
//!
//! Hosts list `available_commands_update` entries in their command menus and
//! send a picked command back as `/name args` prompt text. ACP sessions run
//! every command of [`crate::app::session_commands`], which resolves them,
//! including both built-ins, `/compact` and `/init`;
//! [`crate::app::session_options`] lists them, and this module renders that
//! list for ACP, as the ACP adapter will render the daemon's (#1670).
//!
//! T3 Code fills its `$` menu only from `available_commands_update` entries
//! named `$<skill>` and sends a picked skill back as a `$<skill>` mention in
//! plain prompt text. Rho advertises its discovered skills that way; the
//! shared resolver then loads every mentioned skill before the first model
//! request, as one batch of host-requested `skill` calls.
//!
//! `/rho-wake <token>` runs a T3 wake watcher (see [`super::t3_wake`]). It is
//! ACP's own command, matched before the shared ones, and never advertised:
//! only the host delegates it.

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, UnstructuredCommandInput,
};
use rho_protocol as wire;
use rho_sdk::UserInput;

use super::t3_wake::{WakeSignal, WAKE_COMMAND};
use crate::app::{
    session_commands::{
        self, leading_text, BuiltInCommand, CommandScope, CommandSet, Hosting, PromptCommand,
    },
    session_runtime::CommandRejected,
};

/// The shared commands ACP sessions run: every one.
pub(super) const ACP_COMMANDS: CommandSet = CommandSet {
    built_ins: &[BuiltInCommand::Compact, BuiltInCommand::Init],
    workflows: Hosting::Runs,
    agents: Hosting::Runs,
    sessions_label: "ACP sessions",
};

/// What a prompt asks an ACP session to do.
#[derive(Debug)]
pub(super) enum AcpPrompt {
    /// A command every host runs, or a model turn.
    Shared(PromptCommand),
    /// `/rho-wake <token>`: wait for a parent's wake signal, without a model turn.
    AwaitWake(WakeSignal),
}

/// `commands` as `available_commands_update` entries: skills named
/// `$<skill>`, the rest by name with their argument hint as unstructured
/// input. A command of a kind this rho does not know is left out.
pub(super) fn available_commands(commands: &[wire::SessionCommand]) -> Vec<AvailableCommand> {
    commands.iter().filter_map(advertised).collect()
}

/// Matches ACP's `/rho-wake`, then resolves the shared commands.
pub(super) async fn resolve(
    input: UserInput,
    scope: &CommandScope<'_>,
) -> Result<AcpPrompt, CommandRejected> {
    if let Some(token) = leading_text(&input)
        .and_then(|text| text.strip_prefix(WAKE_COMMAND))
        .and_then(|rest| rest.starts_with(char::is_whitespace).then_some(rest))
    {
        return WakeSignal::parse(token)
            .map(AcpPrompt::AwaitWake)
            .ok_or_else(|| CommandRejected(format!("usage: {WAKE_COMMAND} <token>")));
    }
    session_commands::resolve(input, scope, ACP_COMMANDS)
        .await
        .map(AcpPrompt::Shared)
}

fn advertised(command: &wire::SessionCommand) -> Option<AvailableCommand> {
    let name = match command.kind {
        wire::CommandKind::Slash => command.name.clone(),
        wire::CommandKind::Skill => format!("${}", command.name),
        // `wire::CommandKind` is non-exhaustive: a kind a newer daemon sends.
        _ => return None,
    };
    let advertised = AvailableCommand::new(name, command.description.clone());
    Some(match &command.input_hint {
        Some(hint) => advertised.input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new(hint.clone()),
        )),
        None => advertised,
    })
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
