//! Zen mode collapses each run of hidden work (tool cards plus the reasoning
//! between them) into one summary row, so assistant output written before and
//! after tool use stays visibly separate.
//!
//! A run is a maximal span of consecutive hidden entries. Its first entry paints
//! the summary row; every other hidden entry paints nothing. Only hidden tools
//! are counted, and tools hide only in zen, so callers pass their plain `hides`
//! policy without a separate zen check: outside zen every run has zero tools and
//! paints nothing.

use super::Entry;

/// Summary row painted at `index`, or `None` unless `index` starts a hidden run
/// containing at least one tool.
pub(super) fn summary_row_at(
    entries: &[Entry],
    index: usize,
    hides: impl Fn(&Entry) -> bool,
) -> Option<Entry> {
    let starts_run = hides(entries.get(index)?) && (index == 0 || !hides(&entries[index - 1]));
    if !starts_run {
        return None;
    }
    let tools = entries[index..]
        .iter()
        .take_while(|entry| hides(entry))
        .filter(|entry| matches!(entry, Entry::Tool(_)))
        .count();
    (tools > 0).then(|| summary_entry(tools))
}

/// Start of the hidden run that ends right before `index`, or `index` itself.
///
/// A change at `index` can grow or shrink the run before it, so rebuilding
/// from here refreshes that run's summary row.
pub(super) fn run_start(entries: &[Entry], index: usize, hides: impl Fn(&Entry) -> bool) -> usize {
    index
        - entries[..index]
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
