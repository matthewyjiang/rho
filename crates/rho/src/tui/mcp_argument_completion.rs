//! Server-suggested values for the MCP prompt argument being typed.
//!
//! `completion/complete` is a round-trip, and the command palette matches
//! synchronously on every keystroke, so the two cannot meet directly. They meet
//! through a cache instead: matching only ever reads what has already arrived,
//! while the event loop starts and collects the requests that fill it.
//!
//! The request policy is deliberately clock-free. At most one request is in
//! flight at a time, and the next one is chosen from wherever the cursor ended
//! up once that request lands. Holding a key down therefore costs one round-trip
//! per round-trip rather than one per character, without a debounce timer to
//! tune or a wall-clock delay for tests to wait out. Every finished request is
//! recorded, including a failed one, so a server that cannot answer is asked
//! once per value rather than on every pass of the loop.

use std::{collections::VecDeque, ops::Range};

use super::{App, CommandChoice, CommandChoiceKind, ComposerMode};
use crate::{
    commands,
    tools::mcp::{
        catalog::{McpPrompt, McpPromptArgument},
        McpCompletionSupport,
    },
};

/// How many finished lookups stay available for reuse.
///
/// Every keystroke inside a value makes a new key, so the cache pays off on
/// backspace, on re-reading a value, and on the repeated passes the event loop
/// makes while the composer sits still. A few dozen entries cover that and stop
/// a long session from growing without bound.
const COMPLETION_CACHE_LIMIT: usize = 64;

/// What one `completion/complete` request is for.
///
/// Equality is the cache key: the same four values always describe the same
/// question, so a repeat is answered locally instead of asked again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct McpCompletionKey {
    pub(in crate::tui) server: String,
    pub(in crate::tui) prompt: String,
    pub(in crate::tui) argument: String,
    pub(in crate::tui) typed: String,
}

/// The argument value the cursor sits in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct McpArgumentCursor {
    pub(in crate::tui) key: McpCompletionKey,
    /// Char range of the value within the composer text. Only this range is
    /// rewritten when a suggestion is picked.
    pub(in crate::tui) value: Range<usize>,
}

impl McpArgumentCursor {
    /// What the palette is currently answering, so a move to a different value
    /// starts the selection over instead of leaving it on an unrelated row.
    pub(super) fn palette_identity(&self) -> String {
        let McpCompletionKey {
            server,
            prompt,
            argument,
            typed,
        } = &self.key;
        format!("mcp:{server}:{prompt} {argument}={typed}")
    }
}

/// What the event loop should do about the value under the cursor.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum McpCompletionStep {
    /// Send nothing: the cursor is not in a value, the answer is already
    /// cached, or a request is already in flight.
    Wait,
    /// Send exactly this request.
    Ask(McpCompletionKey),
}

/// Suggestions already fetched, plus the single request in flight.
#[derive(Debug, Default)]
pub(super) struct McpArgumentCompletions {
    /// Insertion-ordered, oldest first, so the cap evicts the oldest entry.
    cache: VecDeque<(McpCompletionKey, Vec<String>)>,
    pending: Option<PendingCompletion>,
}

#[derive(Debug)]
struct PendingCompletion {
    key: McpCompletionKey,
    handle: tokio::task::JoinHandle<Vec<String>>,
}

impl McpArgumentCompletions {
    pub(super) fn suggestions(&self, key: &McpCompletionKey) -> Option<&[String]> {
        self.cache
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, values)| values.as_slice())
    }

    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Decide what to send for the value under the cursor.
    pub(super) fn next_step(&self, wanted: Option<&McpCompletionKey>) -> McpCompletionStep {
        let Some(key) = wanted else {
            return McpCompletionStep::Wait;
        };
        if self.pending.is_some() || self.suggestions(key).is_some() {
            return McpCompletionStep::Wait;
        }
        McpCompletionStep::Ask(key.clone())
    }

    pub(super) fn store(&mut self, key: McpCompletionKey, values: Vec<String>) {
        if self.cache.len() >= COMPLETION_CACHE_LIMIT {
            self.cache.pop_front();
        }
        self.cache.push_back((key, values));
    }

    /// Drop a request nobody will read, on shutdown.
    pub(super) fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.handle.abort();
        }
    }
}

impl App {
    /// Palette rows for the value under the cursor.
    ///
    /// Reads the cache and nothing else, so per-keystroke matching stays local.
    pub(super) fn mcp_argument_choices(&self) -> Vec<CommandChoice> {
        let Some(cursor) = self.mcp_argument_cursor() else {
            return Vec::new();
        };
        argument_choices(&cursor, &self.mcp_argument_completions)
    }

    /// The request the composer currently calls for, if any.
    pub(super) fn mcp_argument_cursor(&self) -> Option<McpArgumentCursor> {
        // Nothing is asked for a value no palette would show it for: a picker
        // or a shell line is holding the composer, or the palette was dismissed
        // for this keystroke. The checks are the cheapest available, and they
        // run before the catalog is touched.
        if !matches!(self.input_ui.composer(), ComposerMode::Input)
            || self.input_ui.shell_mode().is_some()
            || self.input_ui.command_palette_dismissed()
        {
            return None;
        }
        let text = self.input_ui.text();
        let (server, name) = super::mcp_prompt::parse_command(commands::command_prefix(text)?)?;
        let prompt = self
            .mcp_catalog
            .prompts()
            .into_iter()
            .find(|prompt| prompt.server == server && prompt.name == name)?;
        argument_under_cursor(
            text,
            self.input_ui.cursor(),
            &prompt,
            self.mcp_catalog.completion_support(&server),
        )
    }

    /// Collect a finished lookup and start the next one. Returns whether the
    /// palette must redraw.
    pub(super) async fn poll_mcp_argument_completion(&mut self) -> bool {
        let mut redraw = false;
        if let Some(pending) = self
            .mcp_argument_completions
            .pending
            .take_if(|pending| pending.handle.is_finished())
        {
            // A cancelled or panicking task counts as an empty answer. The key
            // is recorded either way, so a server that cannot answer is not
            // asked again on the next pass of the loop.
            let values = pending.handle.await.unwrap_or_default();
            self.mcp_argument_completions.store(pending.key, values);
            redraw = true;
        }

        let wanted = self.mcp_argument_cursor().map(|cursor| cursor.key);
        if let McpCompletionStep::Ask(key) =
            self.mcp_argument_completions.next_step(wanted.as_ref())
        {
            let catalog = self.mcp_catalog.clone();
            let request = key.clone();
            self.mcp_argument_completions.pending = Some(PendingCompletion {
                key,
                handle: tokio::spawn(async move {
                    catalog
                        .complete_prompt_argument(
                            &request.server,
                            &request.prompt,
                            &request.argument,
                            &request.typed,
                        )
                        .await
                }),
            });
        }
        redraw
    }
}

/// Turn whatever has arrived for this value into palette rows.
///
/// Nothing cached and a request that came back empty are the same row set,
/// because a suggestion is help: when the server has none to give, or could not
/// answer at all, the palette says nothing rather than reporting a failure at
/// someone mid-sentence.
pub(super) fn argument_choices(
    cursor: &McpArgumentCursor,
    completions: &McpArgumentCompletions,
) -> Vec<CommandChoice> {
    completions
        .suggestions(&cursor.key)
        .unwrap_or_default()
        .iter()
        .map(|value| CommandChoice {
            usage: value.clone(),
            description: format!(
                "{} · suggested by MCP server `{}`",
                cursor.key.argument, cursor.key.server
            ),
            name: value.clone(),
            kind: CommandChoiceKind::McpPromptArgument {
                value: cursor.value.clone(),
            },
        })
        .collect()
}

/// Which argument value the cursor sits in, for a prompt typed as a command.
///
/// Kept separate from the catalog lookup so the rule can be checked against
/// text and a cursor alone.
pub(super) fn argument_under_cursor(
    text: &str,
    cursor: usize,
    prompt: &McpPrompt,
    support: McpCompletionSupport,
) -> Option<McpArgumentCursor> {
    match support {
        // Asking a server that never declared `completions` can only earn an
        // error, so the cursor is treated as sitting in no value at all.
        McpCompletionSupport::Absent => return None,
        McpCompletionSupport::Declared => {}
    }
    let chars = text.chars().collect::<Vec<_>>();
    if chars.first() != Some(&'/') {
        return None;
    }
    let token_end = chars.iter().position(|ch| ch.is_whitespace())?;
    // Inside the command token the palette is still completing the command
    // itself, and those matches must not be displaced.
    if cursor <= token_end {
        return None;
    }
    let (argument, value) = typed_value(&chars, token_end, cursor, &prompt.arguments)?;
    Some(McpArgumentCursor {
        key: McpCompletionKey {
            server: prompt.server.clone(),
            prompt: prompt.name.clone(),
            argument,
            typed: chars[value.clone()].iter().collect(),
        },
        value,
    })
}

/// Rewrite one char range of the composer, leaving the rest of the line alone.
pub(super) fn replace_value(text: &str, value: &Range<usize>, chosen: &str) -> (String, usize) {
    let chars = text.chars().collect::<Vec<_>>();
    let start = value.start.min(chars.len());
    let end = value.end.clamp(start, chars.len());
    let head = chars[..start].iter().collect::<String>();
    let tail = chars[end..].iter().collect::<String>();
    (
        format!("{head}{chosen}{tail}"),
        start + chosen.chars().count(),
    )
}

/// The declared argument the cursor is filling in, and the char range of its
/// value within `chars`.
fn typed_value(
    chars: &[char],
    token_end: usize,
    cursor: usize,
    arguments: &[McpPromptArgument],
) -> Option<(String, Range<usize>)> {
    // A prompt with exactly one argument takes the whole trailing text as that
    // argument's value, matching how `McpPrompt::parse_arguments` reads it back.
    if let [only] = arguments {
        let start = (token_end..chars.len())
            .find(|index| !chars[*index].is_whitespace())
            .unwrap_or(chars.len());
        let end = chars[start..]
            .iter()
            .rposition(|ch| !ch.is_whitespace())
            .map_or(start, |offset| start + offset + 1);
        return Some((only.name.clone(), start..end));
    }

    let word = word_at(chars, token_end, cursor)?;
    let equals = chars[word.clone()]
        .iter()
        .position(|ch| *ch == '=')
        .map(|offset| word.start + offset)?;
    // Before the `=` the user is still naming the argument, and a name is not
    // something the server offers values for.
    if cursor <= equals {
        return None;
    }
    let argument = chars[word.start..equals].iter().collect::<String>();
    // An argument the server never declared cannot be completed, and asking
    // about it would only earn an error.
    if !arguments.iter().any(|declared| declared.name == argument) {
        return None;
    }
    Some((argument, equals + 1..word.end))
}

/// The whitespace-delimited word the cursor sits in or has just finished,
/// searched only in the text after the command token.
fn word_at(chars: &[char], token_end: usize, cursor: usize) -> Option<Range<usize>> {
    let mut index = token_end;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && !chars[index].is_whitespace() {
            index += 1;
        }
        if (start..=index).contains(&cursor) {
            return Some(start..index);
        }
    }
    None
}

#[cfg(test)]
#[path = "mcp_argument_completion_tests.rs"]
mod tests;
