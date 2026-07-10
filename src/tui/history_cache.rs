use std::{ops::Range, sync::Arc};

use ratatui::text::Line;

use super::{entry_lines, is_tool_entry, markdown::render_markdown, Entry};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CachedCodeBlock {
    pub(super) line: usize,
    pub(super) copy_columns: Range<usize>,
    pub(super) text: Arc<str>,
}

#[derive(Default)]
pub(super) struct HistoryLineCache {
    settings: Option<HistoryLineCacheSettings>,
    lines: Vec<Line<'static>>,
    entry_ranges: Vec<Range<usize>>,
    code_blocks: Vec<CachedCodeBlock>,
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

    pub(super) fn code_blocks(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
    ) -> &[CachedCodeBlock] {
        self.ensure_current(entries, width, max_tool_output_lines);
        &self.code_blocks
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
            self.code_blocks.clear();
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
        self.code_blocks.retain(|block| block.line < line_start);

        let mut previous_was_tool = rebuild_from
            .checked_sub(1)
            .and_then(|index| entries.get(index))
            .is_some_and(is_tool_entry);
        for entry in entries.iter().skip(rebuild_from) {
            let range_start = self.lines.len();
            if previous_was_tool && is_tool_entry(entry) {
                self.lines.push(Line::raw(""));
            }
            let entry_start = self.lines.len();
            self.cache_code_blocks(entry, width, entry_start);
            self.lines
                .extend(entry_lines(entry, width, max_tool_output_lines));
            self.entry_ranges.push(range_start..self.lines.len());
            previous_was_tool = is_tool_entry(entry);
        }
    }

    fn cache_code_blocks(&mut self, entry: &Entry, width: usize, entry_start: usize) {
        let Entry::Assistant(text) = entry else {
            return;
        };
        let inner_width = width.saturating_sub(2).max(1);
        let mut in_code_block = false;
        let rendered = render_markdown(text, inner_width, &mut in_code_block);
        self.code_blocks.extend(
            rendered
                .code_blocks
                .into_iter()
                .map(|block| CachedCodeBlock {
                    // entry_lines adds a blank row before the rendered markdown.
                    line: entry_start.saturating_add(1 + block.top_line),
                    // entry_lines also pads rendered markdown by one column on each side.
                    copy_columns: block.copy_columns.start.saturating_add(1)
                        ..block.copy_columns.end.saturating_add(1),
                    text: Arc::from(block.text),
                }),
        );
    }
}

#[cfg(test)]
#[path = "history_cache_tests.rs"]
mod tests;
