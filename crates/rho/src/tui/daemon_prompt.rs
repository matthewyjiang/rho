//! A composer turn as the `rho/session/prompt` content that runs it in a
//! daemon session (#1667).
//!
//! The daemon resolves a prompt's leading command and loads the `$skill`s
//! its text mentions itself, so the TUI sends its model text as is. Only a
//! skill the TUI would load first, as a host call, needs spelling out: the
//! daemon loads it for its own `/$<name>` command.

use rho_protocol::ContentBlock;

use super::{ChatMedia, TurnPrompt};

/// What a daemon prompt for `turn` with `media` carries: its text, then
/// each attachment.
pub(super) fn prompt_content(turn: &TurnPrompt, media: &[ChatMedia]) -> Vec<ContentBlock> {
    let text = prompt_text(turn);
    let mut content = Vec::with_capacity(1 + media.len());
    if !text.is_empty() {
        content.push(ContentBlock::Text { text });
    }
    content.extend(media.iter().cloned().map(ChatMedia::wire_block));
    content
}

/// `turn`'s model text, led by the daemon's `/$<name>` when the turn loads
/// a skill first, as `/skill:<name>`, `/init`, and the agent creator do: a
/// typed `/skill:<name>` becomes it, and a generated prompt follows it.
fn prompt_text(turn: &TurnPrompt) -> String {
    let Some(skill) = turn.skills.first() else {
        return turn.model.clone();
    };
    let command = format!("/${skill}");
    match turn.model.strip_prefix(&format!("/skill:{skill}")) {
        Some(args) if args.is_empty() || args.starts_with(char::is_whitespace) => {
            format!("{command}{args}")
        }
        _ if turn.model.is_empty() => command,
        _ => format!("{command} {}", turn.model),
    }
}

#[cfg(test)]
#[path = "daemon_prompt_tests.rs"]
mod tests;
