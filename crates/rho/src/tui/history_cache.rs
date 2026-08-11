use std::{ops::Range, sync::Arc};

use ratatui::text::Line;

use super::{
    feed_image::{FeedImage, RenderedImagePlacement, RenderedImagePlacements},
    markdown::incremental_markdown_tail_start,
    markdown_image::MarkdownImageSource,
    message_render::render_assistant_content,
    render::{apply_markdown_images, pad_display_line, render_entry_with_options, TrailingBlank},
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

/// Layout/display inputs that invalidate the history line cache when they change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HistoryRenderSettings {
    pub width: usize,
    pub max_tool_output_lines: usize,
    pub zen_mode: bool,
    /// Active theme generation so palette switches rebuild styled lines.
    pub theme_generation: u64,
    /// Max rows one feed image may reserve inside this layout.
    pub max_image_height: u16,
}

impl HistoryRenderSettings {
    /// Zen mode keeps entry indices stable but contributes no transcript lines for
    /// tool cards or reasoning blocks.
    pub(super) fn hides_entry(self, entry: &Entry) -> bool {
        self.zen_mode && matches!(entry, Entry::Tool(_) | Entry::Reasoning(_))
    }
}

#[derive(Default)]
pub(super) struct HistoryLineCache {
    settings: Option<HistoryRenderSettings>,
    lines: Vec<Line<'static>>,
    entry_ranges: Vec<Range<usize>>,
    assistant_caches: Vec<Option<IncrementalAssistantCache>>,
    code_blocks: Vec<CachedCodeBlock>,
    image_placements: Vec<RenderedImagePlacements>,
    dirty_from: Option<usize>,
    /// Entry indices to re-render in place (height may change). Applied on the
    /// next `ensure_current` without rebuilding the history suffix when the
    /// cache is already warm — used by tool expand/collapse.
    resplice: Vec<usize>,
    appended_assistant: Option<usize>,
    /// When set, the last entry is still being streamed and must not own a trailing blank.
    open_stream_tail: bool,
}

impl HistoryLineCache {
    pub(super) fn invalidate_from(&mut self, index: usize) {
        self.appended_assistant = None;
        // Fold pending surgical marks into the suffix rebuild so an earlier
        // resplice is not lost when a later invalidation starts. Marks at or
        // after `index` are already covered by rebuilding from `index`.
        let mut dirty = index;
        for &resplice_index in &self.resplice {
            dirty = dirty.min(resplice_index);
        }
        self.resplice.clear();
        self.dirty_from = Some(
            self.dirty_from
                .map_or(dirty, |existing| existing.min(dirty)),
        );
    }

    /// Re-render these entries on the next paint without dropping the cached
    /// suffix after them. Falls back to [`Self::invalidate_from`] when the
    /// cache is cold or already suffix-dirty.
    ///
    /// Used when tool cards toggle expand/collapse (height changes, content of
    /// later entries does not).
    pub(super) fn resplice_entries(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.appended_assistant = None;
        if self.dirty_from.is_some() {
            // Already doing a suffix rebuild; fold the earliest index into it.
            for index in indices {
                self.dirty_from = Some(self.dirty_from.map_or(index, |dirty| dirty.min(index)));
            }
            return;
        }
        self.resplice.extend(indices);
    }

    /// Suppress the trailing separator on the last entry while a stream is still open.
    ///
    /// Flipping this flag rebuilds only the last cached entry so live continuations can
    /// abut committed content, then restore normal spacing when the stream finishes.
    pub(super) fn set_open_stream_tail(&mut self, open: bool) {
        if self.open_stream_tail == open {
            return;
        }
        self.open_stream_tail = open;
        if let Some(last) = self.entry_ranges.len().checked_sub(1) {
            // Surgical: only the last entry's trailing blank changes.
            self.resplice_entries([last]);
        }
    }

    pub(super) fn assistant_appended(&mut self, index: usize) {
        let can_extend = index + 1 == self.entry_ranges.len()
            && self.dirty_from.is_none()
            && self.resplice.is_empty()
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
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) -> usize {
        self.ensure_current(entries, settings, image_resolver);
        self.lines.len()
    }

    pub(super) fn code_blocks(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) -> &[CachedCodeBlock] {
        self.ensure_current(entries, settings, image_resolver);
        &self.code_blocks
    }

    pub(super) fn entry_index_at_line(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        line: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> Option<usize> {
        self.ensure_current(entries, settings, image_resolver);
        self.entry_ranges
            .iter()
            .position(|range| range.contains(&line))
    }

    pub(super) fn extend_visible_lines(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        slice: HistoryLineSlice,
        target: &mut Vec<Line<'static>>,
        image_resolver: EntryImageResolver<'_>,
    ) {
        if slice.count == 0 {
            return;
        }

        self.ensure_current(entries, settings, image_resolver);
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
        settings: HistoryRenderSettings,
        start: usize,
        count: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> Vec<super::feed_image::VisibleImagePlacement> {
        self.ensure_current(entries, settings, image_resolver);
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
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) {
        if self.settings != Some(settings) {
            self.settings = Some(settings);
            self.lines.clear();
            self.entry_ranges.clear();
            self.assistant_caches.clear();
            self.code_blocks.clear();
            self.image_placements.clear();
            self.appended_assistant = None;
            self.resplice.clear();
            self.dirty_from = Some(0);
        }

        match entries.len().cmp(&self.entry_ranges.len()) {
            std::cmp::Ordering::Less => self.invalidate_from(entries.len()),
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Greater => self.invalidate_from(self.entry_ranges.len()),
        }

        // Prefer surgical resplice when the cache is warm and only discrete
        // entries changed height (tool expand/collapse). Fall back to a suffix
        // rebuild if anything looks inconsistent.
        if self.dirty_from.is_none() && !self.resplice.is_empty() {
            let mut indices = std::mem::take(&mut self.resplice);
            indices.sort_unstable();
            indices.dedup();
            if !self.try_resplice_entries(entries, &indices, settings, image_resolver) {
                // try_resplice may have already forced a full rebuild (dirty 0).
                if self.dirty_from.is_none() {
                    let min = indices.first().copied().unwrap_or(0);
                    self.dirty_from = Some(min);
                }
            }
        } else if !self.resplice.is_empty() {
            // Suffix rebuild wins; fold pending marks into the earliest dirty
            // index so they are not dropped before the rebuild.
            for index in self.resplice.drain(..) {
                self.dirty_from = Some(self.dirty_from.map_or(index, |dirty| dirty.min(index)));
            }
        }

        let Some(dirty_from) = self.dirty_from.take() else {
            return;
        };
        let rebuild_from = dirty_from.min(entries.len()).min(self.entry_ranges.len());
        if self.appended_assistant.take() == Some(rebuild_from)
            && self.try_extend_last_assistant(entries, rebuild_from, settings.width)
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

        for (entry_index, entry) in entries.iter().enumerate().skip(rebuild_from) {
            self.push_rendered_entry(entry_index, entry, entries.len(), settings, image_resolver);
        }
    }

    /// Re-render `indices` in place and shift later line offsets by the height
    /// delta. Returns false when the cache cannot support a surgical update.
    fn try_resplice_entries(
        &mut self,
        entries: &[Entry],
        indices: &[usize],
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) -> bool {
        if indices.is_empty() {
            return true;
        }
        if self.entry_ranges.len() != entries.len()
            || self.assistant_caches.len() != entries.len()
            || self.settings != Some(settings)
        {
            return false;
        }
        for &index in indices {
            if index >= entries.len() || index >= self.entry_ranges.len() {
                return false;
            }
        }

        for &index in indices {
            let old_range = self.entry_ranges[index].clone();
            if old_range.end > self.lines.len() || old_range.start > old_range.end {
                return false;
            }

            let entry = &entries[index];
            let (new_lines, new_code_blocks, new_image) = match prepare_cache_entry_render(
                entry,
                index,
                entries.len(),
                settings,
                self.open_stream_tail,
                image_resolver,
            ) {
                None => (Vec::new(), Vec::new(), None),
                Some(rendered) => (
                    rendered.lines,
                    rendered.code_blocks,
                    rendered.image_placement,
                ),
            };

            let start = old_range.start;
            let end = old_range.end;
            let old_len = end - start;
            let new_len = new_lines.len();
            let delta = new_len as isize - old_len as isize;

            self.lines.splice(start..end, new_lines);

            // Code blocks inside the old span are replaced; later ones shift.
            self.code_blocks
                .retain(|block| block.line < start || block.line >= end);
            for block in &mut self.code_blocks {
                if block.line >= end {
                    block.line = offset_usize(block.line, delta);
                }
            }
            self.code_blocks
                .extend(new_code_blocks.into_iter().map(|mut block| {
                    block.line = start.saturating_add(block.line);
                    block
                }));
            // Keep code_blocks ordered by line for stable hit-testing.
            self.code_blocks.sort_by_key(|block| block.line);

            self.image_placements = shift_image_placements_for_splice(
                &self.image_placements,
                start,
                end,
                delta,
                new_image.map(|placement| placement.offset_rows(start)),
            );

            self.entry_ranges[index] = start..start + new_len;
            for range in self.entry_ranges.iter_mut().skip(index + 1) {
                range.start = offset_usize(range.start, delta);
                range.end = offset_usize(range.end, delta);
            }

            // Tool/reasoning toggles never own incremental assistant state.
            // Last-assistant open-stream blank changes also clear it; rebuild
            // only if this is still the last assistant entry.
            self.assistant_caches[index] = None;
            if index + 1 == entries.len() {
                if let Entry::Assistant(text) = entry {
                    let stable_source_len = incremental_markdown_tail_start(text);
                    let stable_line_count = if stable_source_len == 0 {
                        0
                    } else {
                        render_assistant_content(&text[..stable_source_len], settings.width)
                            .lines
                            .len()
                    };
                    self.assistant_caches[index] = Some(IncrementalAssistantCache {
                        stable_source_len,
                        stable_line_count,
                    });
                }
            }
        }

        // Sanity: flat line buffer length matches last range end.
        let ok = match self.entry_ranges.last() {
            Some(last) => last.end == self.lines.len(),
            None => self.lines.is_empty(),
        };
        if !ok {
            // Should be unreachable if ranges stayed coherent; force a full
            // rebuild rather than paint a torn cache.
            self.lines.clear();
            self.entry_ranges.clear();
            self.assistant_caches.clear();
            self.code_blocks.clear();
            self.image_placements.clear();
            self.dirty_from = Some(0);
            return false;
        }
        true
    }

    fn push_rendered_entry(
        &mut self,
        entry_index: usize,
        entry: &Entry,
        entries_len: usize,
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) {
        let range_start = self.lines.len();
        let Some(rendered) = prepare_cache_entry_render(
            entry,
            entry_index,
            entries_len,
            settings,
            self.open_stream_tail,
            image_resolver,
        ) else {
            // Keep a zero-height range so entry indices stay aligned with history.
            self.entry_ranges.push(range_start..range_start);
            self.assistant_caches.push(None);
            return;
        };

        let entry_start = self.lines.len();
        self.code_blocks
            .extend(rendered.code_blocks.into_iter().map(|mut block| {
                // Content starts at the first entry row; trailing blank is after.
                block.line = entry_start.saturating_add(block.line);
                block
            }));
        if let Some(placement) = rendered.image_placement {
            self.image_placements
                .push(placement.offset_rows(entry_start));
        }
        self.lines.extend(rendered.lines);
        self.entry_ranges.push(range_start..self.lines.len());
        // Only the last entry can be appended to, so only its cache is ever
        // read (see `assistant_appended`). Building one for every entry would
        // re-render each assistant message's stable prefix a second time,
        // doubling the markdown work on every resize.
        let is_last = entry_index + 1 == entries_len;
        self.assistant_caches.push(match entry {
            Entry::Assistant(text) if is_last => {
                let stable_source_len = incremental_markdown_tail_start(text);
                let stable_line_count = if stable_source_len == 0 {
                    0
                } else {
                    render_assistant_content(&text[..stable_source_len], settings.width)
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

        // Open stream tails omit the trailing separator; closed entries keep it.
        let has_trailing_blank = !(self.open_stream_tail && index + 1 == entries.len());
        let content_end = if has_trailing_blank {
            range.end.saturating_sub(1)
        } else {
            range.end
        };
        // Content starts at range.start; there is no leading spacer.
        let preserve_end = range.start.saturating_add(cache.stable_line_count);
        if preserve_end >= content_end || preserve_end > self.lines.len() {
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
        let trailing_blank = has_trailing_blank.then(|| self.lines[range.end - 1].clone());
        self.lines.truncate(preserve_end);
        self.code_blocks.retain(|block| block.line < preserve_end);

        let previous_stable_source_len = cache.stable_source_len;
        self.append_assistant_segment(&text[previous_stable_source_len..new_tail_start], width);
        let cache = self.assistant_caches[index]
            .as_mut()
            .expect("assistant cache exists");
        cache.stable_line_count = self.lines.len().saturating_sub(range.start);
        cache.stable_source_len = new_tail_start;
        self.append_assistant_segment(&text[new_tail_start..], width);
        if let Some(trailing_blank) = trailing_blank {
            self.lines.push(trailing_blank);
        }
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
            .extend(rendered.lines.into_iter().map(pad_display_line));
    }
}

fn offset_usize(value: usize, delta: isize) -> usize {
    if delta >= 0 {
        value.saturating_add(delta as usize)
    } else {
        value.saturating_sub((-delta) as usize)
    }
}

/// Shared entry render for full rebuild and surgical resplice paths.
///
/// Returns `None` for hidden entries. Code-block line numbers and image
/// placements are relative to the entry start; callers relocate them.
struct PreparedCacheEntry {
    lines: Vec<Line<'static>>,
    code_blocks: Vec<CachedCodeBlock>,
    image_placement: Option<RenderedImagePlacements>,
}

fn prepare_cache_entry_render(
    entry: &Entry,
    entry_index: usize,
    entries_len: usize,
    settings: HistoryRenderSettings,
    open_stream_tail: bool,
    image_resolver: EntryImageResolver<'_>,
) -> Option<PreparedCacheEntry> {
    if settings.hides_entry(entry) {
        return None;
    }
    let trailing_blank = if open_stream_tail && entry_index + 1 == entries_len {
        TrailingBlank::Omit
    } else {
        TrailingBlank::Include
    };
    let mut rendered = render_entry_with_options(
        entry,
        settings.width,
        settings.max_tool_output_lines,
        settings.max_image_height,
        trailing_blank,
    );
    if !rendered.image_sources.is_empty() {
        let images = image_resolver(entry_index, &rendered.image_sources);
        apply_markdown_images(
            &mut rendered,
            &images,
            settings.width,
            settings.max_image_height,
        );
    }
    let code_blocks = rendered
        .code_blocks
        .into_iter()
        .map(|block| CachedCodeBlock {
            // Relative to entry start; callers offset when placing.
            line: block.top_line,
            // render_entry also pads markdown by one column on each side.
            copy_columns: block.copy_columns.start.saturating_add(1)
                ..block.copy_columns.end.saturating_add(1),
            text: Arc::from(block.text),
        })
        .collect();
    Some(PreparedCacheEntry {
        lines: rendered.lines,
        code_blocks,
        image_placement: rendered.image_placement,
    })
}

/// Drop placements overlapping `[start, end)`, shift those at/after `end` by
/// `delta`, then append any replacement placement for the respliced entry.
fn shift_image_placements_for_splice(
    existing: &[RenderedImagePlacements],
    start: usize,
    end: usize,
    delta: isize,
    replacement: Option<RenderedImagePlacements>,
) -> Vec<RenderedImagePlacements> {
    let mut out = Vec::with_capacity(existing.len() + usize::from(replacement.is_some()));
    for group in existing {
        let placements: Vec<_> = group
            .iter()
            .filter_map(|placement| {
                if placement.rows.end <= start {
                    Some(placement.clone())
                } else if placement.rows.start >= end {
                    let offset_start = offset_usize(placement.rows.start, delta);
                    let offset_end = offset_usize(placement.rows.end, delta);
                    Some(RenderedImagePlacement {
                        image: placement.image.clone(),
                        rows: offset_start..offset_end,
                    })
                } else {
                    None
                }
            })
            .collect();
        if !placements.is_empty() {
            out.push(RenderedImagePlacements::from_placements(placements));
        }
    }
    if let Some(replacement) = replacement {
        out.push(replacement);
    }
    out
}

#[cfg(test)]
#[path = "history_cache_tests.rs"]
mod tests;
