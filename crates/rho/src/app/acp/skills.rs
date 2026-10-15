//! `$skill` commands for ACP hosts.
//!
//! T3 Code fills its `$` menu only from `available_commands_update` entries
//! named `$<skill>` and sends a picked skill back as a `$<skill>` mention in
//! plain prompt text. Rho advertises its discovered skills that way, then
//! loads the first mentioned skill with a host-requested `skill` call before
//! the first model request, like the TUI's `/skill:<name>`. The command
//! catalog in [`super::commands`] owns when these apply.

use std::sync::LazyLock;

use agent_client_protocol::schema::v1::AvailableCommand;
use rho_sdk::{
    model::{ContentBlock, ToolCall},
    UserInput,
};

use crate::skills::Skill;

/// Tool that loads a skill. Without it, neither advertising nor mentions apply.
pub(super) const SKILL_TOOL: &str = "skill";

/// One whitespace-delimited mention: any currency sigil, then a name. This is
/// T3's `SKILL_MENTION_PATTERN` (`packages/shared/src/composerInlineTokens.ts`)
/// with its `(^|\s)` / `(?=\s|$)` delimiters handled by splitting on
/// whitespace. Its letter and money rules live in [`mentionable`].
static MENTION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\p{Sc}([a-zA-Z0-9][a-zA-Z0-9:_-]*)$").expect("mention pattern is valid")
});

/// `$<name>` commands for every skill a T3 composer can mention.
pub(super) fn available_commands(skills: &[Skill]) -> Vec<AvailableCommand> {
    skills
        .iter()
        .filter(|skill| mentionable(&skill.name))
        .map(|skill| AvailableCommand::new(format!("${}", skill.name), skill.description.clone()))
        .collect()
}

/// The first known skill mentioned in the prompt's text blocks. `discover`
/// runs only when the prompt has a mention. Only one skill loads up front
/// because a run starts with at most one host call; later mentions stay in
/// the prompt for the model.
pub(super) fn first_mention(
    input: &UserInput,
    discover: impl FnOnce() -> Vec<Skill>,
) -> Option<Skill> {
    let names = input
        .blocks()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .flat_map(|text| text.split(char::is_whitespace))
        .filter_map(|token| MENTION.captures(token)?.get(1))
        .map(|name| name.as_str())
        .filter(|name| mentionable(name))
        .collect::<Vec<_>>();
    if names.is_empty() {
        return None;
    }
    let mut skills = discover();
    let index = names
        .into_iter()
        .find_map(|name| skills.iter().position(|skill| skill.name == name))?;
    Some(skills.swap_remove(index))
}

/// Host-requested call that loads skill `name`, matching the TUI's `/skill:`.
pub(super) fn load_call(name: &str) -> ToolCall {
    ToolCall {
        id: rho_sdk::ToolCallId::new().into_string(),
        name: SKILL_TOOL.into(),
        arguments: serde_json::json!({ "name": name }),
    }
}

/// T3 keeps `$20`, `$20k`, `$1e6`, and other amounts as prose, so a name
/// needs a letter and must not read as a number with a unit or exponent.
fn mentionable(name: &str) -> bool {
    name.bytes().any(|byte| byte.is_ascii_alphabetic()) && !reads_as_amount(name)
}

/// `[0-9][0-9_]*` optionally followed by one `kmbt` unit or `e<digits>`.
fn reads_as_amount(name: &str) -> bool {
    let digits_end = name
        .bytes()
        .position(|byte| !(byte.is_ascii_digit() || byte == b'_'))
        .unwrap_or(name.len());
    if digits_end == 0 || !name.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    match name.as_bytes()[digits_end..] {
        [] => true,
        [unit] => matches!(unit.to_ascii_lowercase(), b'k' | b'm' | b'b' | b't'),
        [exponent, ref power @ ..] => {
            exponent.eq_ignore_ascii_case(&b'e') && power.iter().all(u8::is_ascii_digit)
        }
    }
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
