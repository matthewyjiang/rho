//! Zen mode collapses each run of hidden work (tool cards plus the reasoning
//! between them) into one summary row, so assistant output written before and
//! after tool use stays visibly separate.
//!
//! A run is a maximal span of consecutive hidden entries. Its summary row is
//! anchored on the run's first tool entry; every other hidden entry paints
//! nothing. Callers pass their own `hides` policy and only use these helpers
//! while zen is on.

use super::Entry;

/// Tool count for the summary row anchored at `index`.
///
/// `None` unless `index` is a hidden tool with no earlier hidden tool in the
/// same run.
pub(super) fn summary_at(
    entries: &[Entry],
    index: usize,
    hides: impl Fn(&Entry) -> bool,
) -> Option<usize> {
    let entry = entries.get(index)?;
    if !matches!(entry, Entry::Tool(_)) || !hides(entry) {
        return None;
    }
    let earlier_tool = entries[..index]
        .iter()
        .rev()
        .take_while(|entry| hides(entry))
        .any(|entry| matches!(entry, Entry::Tool(_)));
    if earlier_tool {
        return None;
    }
    Some(
        entries[index..]
            .iter()
            .take_while(|entry| hides(entry))
            .filter(|entry| matches!(entry, Entry::Tool(_)))
            .count(),
    )
}

/// Start of the hidden run that ends right before `index`, or `index` itself.
///
/// A change at `index` can grow or shrink the run before it, so rebuilding
/// from here refreshes that run's anchored count.
pub(super) fn run_start(entries: &[Entry], index: usize, hides: impl Fn(&Entry) -> bool) -> usize {
    let end = index.min(entries.len());
    end - entries[..end]
        .iter()
        .rev()
        .take_while(|entry| hides(entry))
        .count()
}

/// Summary row painted in place of a run of `tools` hidden tool cards.
pub(super) fn summary_entry(tools: usize) -> Entry {
    let noun = if tools == 1 {
        "tool call"
    } else {
        "tool calls"
    };
    Entry::Notice(format!("⋯ {tools} {noun}"))
}

#[cfg(test)]
#[path = "zen_tool_run_tests.rs"]
mod tests;
