use std::ops::Range;

use ratatui::text::Line;

use super::{entry_lines, is_tool_entry, Entry};

#[derive(Default)]
pub(super) struct HistoryLineCache {
    settings: Option<HistoryLineCacheSettings>,
    lines: Vec<Line<'static>>,
    entry_ranges: Vec<Range<usize>>,
    dirty_from: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HistoryLineCacheSettings {
    width: usize,
    max_tool_output_lines: usize,
}

impl HistoryLineCache {
    pub(super) fn invalidate_from(&mut self, index: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(index, |dirty| dirty.min(index)));
    }

    pub(super) fn line_count(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
    ) -> usize {
        self.ensure_current(entries, width, max_tool_output_lines);
        self.lines.len()
    }

    pub(super) fn extend_visible_lines(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        start: usize,
        count: usize,
        target: &mut Vec<Line<'static>>,
    ) {
        if count == 0 {
            return;
        }

        self.ensure_current(entries, width, max_tool_output_lines);
        let end = start.saturating_add(count).min(self.lines.len());
        if start >= end {
            return;
        }
        target.extend(self.lines[start..end].iter().cloned());
    }

    fn ensure_current(&mut self, entries: &[Entry], width: usize, max_tool_output_lines: usize) {
        let settings = HistoryLineCacheSettings {
            width,
            max_tool_output_lines,
        };
        if self.settings != Some(settings) {
            self.settings = Some(settings);
            self.lines.clear();
            self.entry_ranges.clear();
            self.dirty_from = Some(0);
        }

        match entries.len().cmp(&self.entry_ranges.len()) {
            std::cmp::Ordering::Less => self.invalidate_from(entries.len()),
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Greater => self.invalidate_from(self.entry_ranges.len()),
        }

        let Some(dirty_from) = self.dirty_from.take() else {
            return;
        };
        let rebuild_from = dirty_from.min(entries.len()).min(self.entry_ranges.len());
        let line_start = if rebuild_from == 0 {
            0
        } else {
            self.entry_ranges[rebuild_from - 1].end
        };
        self.lines.truncate(line_start);
        self.entry_ranges.truncate(rebuild_from);

        let mut previous_was_tool = rebuild_from
            .checked_sub(1)
            .and_then(|index| entries.get(index))
            .is_some_and(is_tool_entry);
        for entry in entries.iter().skip(rebuild_from) {
            let start = self.lines.len();
            if previous_was_tool && is_tool_entry(entry) {
                self.lines.push(Line::raw(""));
            }
            self.lines
                .extend(entry_lines(entry, width, max_tool_output_lines));
            self.entry_ranges.push(start..self.lines.len());
            previous_was_tool = is_tool_entry(entry);
        }
    }
}
