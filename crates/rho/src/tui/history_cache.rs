use std::{ops::Range, sync::Arc};

use ratatui::text::Line;

use super::{
    feed_image::{FeedImage, RenderedImagePlacements},
    is_tool_entry,
    markdown::incremental_markdown_tail_start,
    markdown_image::MarkdownImageSource,
    message_render::render_assistant_content,
    render::{apply_markdown_images, pad_entry_line, render_entry},
    Entry,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CachedCodeBlock {
    pub(super) line: usize,
    pub(super) copy_columns: Range<usize>,
    pub(super) text: Arc<str>,
}

#[derive(Clone, Copy, Debug)]
struct IncrementalAssistantCache {
    stable_source_len: usize,
    stable_line_count: usize,
}

/// Resolves loaded `FeedImage`s for the image references of one entry.
/// Each tuple retains its index in `sources`.
pub(super) type EntryImageResolver<'a> =
    &'a dyn Fn(usize, &[MarkdownImageSource]) -> Vec<(usize, FeedImage)>;

#[derive(Clone, Copy, Debug)]
pub(super) struct HistoryLineSlice {
    pub(super) start: usize,
    pub(super) count: usize,
}

#[derive(Default)]
pub(super) struct HistoryLineCache {
    settings: Option<HistoryLineCacheSettings>,
    lines: Vec<Line<'static>>,
    entry_ranges: Vec<Range<usize>>,
    assistant_caches: Vec<Option<IncrementalAssistantCache>>,
    code_blocks: Vec<CachedCodeBlock>,
    image_placements: Vec<RenderedImagePlacements>,
    dirty_from: Option<usize>,
    appended_assistant: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HistoryLineCacheSettings {
    width: usize,
    max_tool_output_lines: usize,
}

impl HistoryLineCache {
    pub(super) fn invalidate_from(&mut self, index: usize) {
        self.appended_assistant = None;
        self.dirty_from = Some(self.dirty_from.map_or(index, |dirty| dirty.min(index)));
    }

    pub(super) fn assistant_appended(&mut self, index: usize) {
        let can_extend = index + 1 == self.entry_ranges.len()
            && self.dirty_from.is_none()
            && self
                .assistant_caches
                .get(index)
                .is_some_and(Option::is_some);
        if can_extend {
            self.appended_assistant = Some(index);
            self.dirty_from = Some(index);
        } else {
            self.invalidate_from(index);
        }
    }

    pub(super) fn line_count(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> usize {
        self.ensure_current(entries, width, max_tool_output_lines, image_resolver);
        self.lines.len()
    }

    pub(super) fn code_blocks(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> &[CachedCodeBlock] {
        self.ensure_current(entries, width, max_tool_output_lines, image_resolver);
        &self.code_blocks
    }

    pub(super) fn entry_index_at_line(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        line: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> Option<usize> {
        self.ensure_current(entries, width, max_tool_output_lines, image_resolver);
        self.entry_ranges
            .iter()
            .position(|range| range.contains(&line))
    }

    pub(super) fn extend_visible_lines(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        slice: HistoryLineSlice,
        target: &mut Vec<Line<'static>>,
        image_resolver: EntryImageResolver<'_>,
    ) {
        if slice.count == 0 {
            return;
        }

        self.ensure_current(entries, width, max_tool_output_lines, image_resolver);
        let end = slice
            .start
            .saturating_add(slice.count)
            .min(self.lines.len());
        if slice.start >= end {
            return;
        }
        target.extend(self.lines[slice.start..end].iter().cloned());
    }

    pub(super) fn visible_image_placements(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        start: usize,
        count: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> Vec<super::feed_image::VisibleImagePlacement> {
        self.ensure_current(entries, width, max_tool_output_lines, image_resolver);
        let end = start.saturating_add(count);
        self.image_placements
            .iter()
            .flat_map(|placements| placements.iter())
            .filter_map(|placement| {
                let visible_start = placement.rows.start.max(start);
                let visible_end = placement.rows.end.min(end);
                (visible_start == placement.rows.start && visible_end == placement.rows.end).then(
                    || super::feed_image::VisibleImagePlacement {
                        image: placement.image.clone(),
                        row: visible_start - start,
                        height: visible_end - visible_start,
                    },
                )
            })
            .collect()
    }

    fn ensure_current(
        &mut self,
        entries: &[Entry],
        width: usize,
        max_tool_output_lines: usize,
        image_resolver: EntryImageResolver<'_>,
    ) {
        let settings = HistoryLineCacheSettings {
            width,
            max_tool_output_lines,
        };
        if self.settings != Some(settings) {
            self.settings = Some(settings);
            self.lines.clear();
            self.entry_ranges.clear();
            self.assistant_caches.clear();
            self.code_blocks.clear();
            self.image_placements.clear();
            self.appended_assistant = None;
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
        if self.appended_assistant.take() == Some(rebuild_from)
            && self.try_extend_last_assistant(entries, rebuild_from, width)
        {
            return;
        }
        let line_start = if rebuild_from == 0 {
            0
        } else {
            self.entry_ranges[rebuild_from - 1].end
        };
        self.lines.truncate(line_start);
        self.entry_ranges.truncate(rebuild_from);
        self.assistant_caches.truncate(rebuild_from);
        self.code_blocks.retain(|block| block.line < line_start);
        self.image_placements = self
            .image_placements
            .iter()
            .filter_map(|placements| placements.retain_starting_before(line_start))
            .collect();

        let mut previous_was_tool = rebuild_from
            .checked_sub(1)
            .and_then(|index| entries.get(index))
            .is_some_and(is_tool_entry);
        for (entry_index, entry) in entries.iter().enumerate().skip(rebuild_from) {
            let range_start = self.lines.len();
            if previous_was_tool && is_tool_entry(entry) {
                self.lines.push(Line::raw(""));
            }
            let entry_start = self.lines.len();
            let mut rendered = render_entry(entry, width, max_tool_output_lines);
            if !rendered.image_sources.is_empty() {
                let images = image_resolver(entry_index, &rendered.image_sources);
                apply_markdown_images(&mut rendered, &images, width);
            }
            self.code_blocks
                .extend(
                    rendered
                        .code_blocks
                        .into_iter()
                        .map(|block| CachedCodeBlock {
                            // render_entry adds a blank row before the rendered markdown.
                            line: entry_start.saturating_add(1 + block.top_line),
                            // render_entry also pads markdown by one column on each side.
                            copy_columns: block.copy_columns.start.saturating_add(1)
                                ..block.copy_columns.end.saturating_add(1),
                            text: Arc::from(block.text),
                        }),
                );
            if let Some(placement) = rendered.image_placement {
                self.image_placements
                    .push(placement.offset_rows(entry_start));
            }
            self.lines.extend(rendered.lines);
            self.entry_ranges.push(range_start..self.lines.len());
            self.assistant_caches.push(match entry {
                Entry::Assistant(text) => {
                    let stable_source_len = incremental_markdown_tail_start(text);
                    let stable_line_count = if stable_source_len == 0 {
                        0
                    } else {
                        render_assistant_content(&text[..stable_source_len], width)
                            .lines
                            .len()
                    };
                    Some(IncrementalAssistantCache {
                        stable_source_len,
                        stable_line_count,
                    })
                }
                _ => None,
            });
            previous_was_tool = is_tool_entry(entry);
        }
    }

    fn try_extend_last_assistant(&mut self, entries: &[Entry], index: usize, width: usize) -> bool {
        let Some(Entry::Assistant(text)) = entries.get(index) else {
            return false;
        };
        let Some(cache) = self
            .assistant_caches
            .get_mut(index)
            .and_then(Option::as_mut)
        else {
            return false;
        };
        let Some(range) = self.entry_ranges.get(index).cloned() else {
            return false;
        };
        if cache.stable_source_len > text.len() {
            return false;
        }
        let mutable_source = &text[cache.stable_source_len..];
        if !super::markdown_image::collect_markdown_image_sources(mutable_source).is_empty() {
            return false;
        }
        let new_tail_start = cache
            .stable_source_len
            .saturating_add(incremental_markdown_tail_start(mutable_source));
        if new_tail_start > text.len() || range.end <= range.start {
            return false;
        }

        let preserve_end = range
            .start
            .saturating_add(1)
            .saturating_add(cache.stable_line_count);
        if preserve_end >= range.end || preserve_end > self.lines.len() {
            return false;
        }
        if self
            .image_placements
            .iter()
            .flat_map(|placements| placements.iter())
            .any(|placement| placement.rows.start < range.end && range.start < placement.rows.end)
        {
            return false;
        }
        let trailing_blank = self.lines[range.end - 1].clone();
        self.lines.truncate(preserve_end);
        self.code_blocks.retain(|block| block.line < preserve_end);

        let previous_stable_source_len = cache.stable_source_len;
        self.append_assistant_segment(&text[previous_stable_source_len..new_tail_start], width);
        let cache = self.assistant_caches[index]
            .as_mut()
            .expect("assistant cache exists");
        cache.stable_line_count = self.lines.len().saturating_sub(range.start + 1);
        cache.stable_source_len = new_tail_start;
        self.append_assistant_segment(&text[new_tail_start..], width);
        self.lines.push(trailing_blank);
        self.entry_ranges[index].end = self.lines.len();
        true
    }

    fn append_assistant_segment(&mut self, text: &str, width: usize) {
        if text.is_empty() {
            return;
        }
        let line_start = self.lines.len();
        let rendered = render_assistant_content(text, width);
        self.code_blocks.extend(
            rendered
                .code_blocks
                .into_iter()
                .map(|block| CachedCodeBlock {
                    line: line_start + block.top_line,
                    copy_columns: block.copy_columns.start.saturating_add(1)
                        ..block.copy_columns.end.saturating_add(1),
                    text: Arc::from(block.text),
                }),
        );
        self.lines
            .extend(rendered.lines.into_iter().map(pad_entry_line));
    }
}

#[cfg(test)]
#[path = "history_cache_tests.rs"]
mod tests;
