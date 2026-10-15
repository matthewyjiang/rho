//! `$skill` mentions in prompt text.
//!
//! A whitespace-delimited `$<name>` token that names a known skill loads that
//! skill before the turn's first model request, through a host-requested
//! `skill` call. The rule is T3 Code's chip pattern, so ACP hosts and the TUI
//! agree on what counts as a mention. Callers decide when mentions apply and
//! which skills load ahead of them.

use std::{collections::BTreeSet, sync::LazyLock};

use rho_sdk::model::ToolCall;

use super::Skill;

/// Tool that loads a skill. Without it, mentions stay prose.
pub(crate) const SKILL_TOOL: &str = "skill";

/// One whitespace-delimited mention: any currency sigil, then a name. This is
/// T3's `SKILL_MENTION_PATTERN` (`packages/shared/src/composerInlineTokens.ts`)
/// with its `(^|\s)` / `(?=\s|$)` delimiters handled by splitting on
/// whitespace. Its letter and money rules live in [`mentionable`].
static MENTION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\p{Sc}([a-zA-Z0-9][a-zA-Z0-9:_-]*)$").expect("mention pattern is valid")
});

/// Skills a turn loads: `leading` first, then every known skill the `texts`
/// mention, each skill once and in that order. `discover` runs only when a
/// text has a mention.
pub(crate) fn skills_to_load<'a>(
    leading: &[&'a str],
    texts: &[&'a str],
    discover: impl FnOnce() -> Vec<Skill>,
) -> Vec<&'a str> {
    let mut seen = BTreeSet::new();
    leading
        .iter()
        .copied()
        .chain(mentions(texts, discover))
        .filter(|name| seen.insert(*name))
        .collect()
}

/// Names of the known skills mentioned in `texts`, in mention order.
fn mentions<'a>(texts: &[&'a str], discover: impl FnOnce() -> Vec<Skill>) -> Vec<&'a str> {
    let names = texts
        .iter()
        .flat_map(|text| text.split(char::is_whitespace))
        .filter_map(|token| MENTION.captures(token)?.get(1))
        .map(|name| name.as_str())
        .filter(|name| mentionable(name))
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Vec::new();
    }
    let skills = discover();
    names
        .into_iter()
        .filter(|name| skills.iter().any(|skill| skill.name == *name))
        .collect()
}

/// Host-requested call that loads skill `name`.
pub(crate) fn load_call(name: &str) -> ToolCall {
    ToolCall {
        id: rho_sdk::ToolCallId::new().into_string(),
        name: SKILL_TOOL.into(),
        arguments: serde_json::json!({ "name": name }),
    }
}

/// T3 keeps `$20`, `$20k`, `$1e6`, and other amounts as prose, so a name
/// needs a letter and must not read as a number with a unit or exponent.
pub(crate) fn mentionable(name: &str) -> bool {
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
#[path = "mention_tests.rs"]
mod tests;
