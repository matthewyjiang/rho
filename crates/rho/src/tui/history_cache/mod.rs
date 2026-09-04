use std::{ops::Range, sync::Arc};

use ratatui::text::Line;

mod incremental;

use incremental::IncrementalEntryCache;

use super::{
    feed_image::{FeedImage, RenderedImagePlacements},
    history_soft_settings::SoftSettingsDelta,
    markdown_image::MarkdownImageSource,
    message_render::{render_assistant_content, render_reasoning_content},
    render::{apply_markdown_images, pad_display_line, render_entry_with_options, TrailingBlank},
    rendered_entry::RenderedEntry,
    Entry,
};

/// Content renderer for an entry kind that supports incremental appends.
pub(super) type EntryContentRender = fn(&str, usize) -> RenderedEntry;

/// Streaming text and its renderer when `entry` can extend in place.
///
/// Assistant entries qualify until their turn duration lands. Reasoning
/// entries qualify until their thought duration lands. Either summary appends
/// a suffix line and re-renders once.
pub(super) fn incremental_entry_source(entry: &Entry) -> Option<(&str, EntryContentRender)> {
    match entry {
        Entry::Assistant(assistant) if assistant.worked_for.is_none() => {
            Some((&assistant.text, render_assistant_content))
        }
        Entry::Reasoning(reasoning) if reasoning.thought_for.is_none() => {
            Some((&reasoning.text, render_reasoning_content))
        }
        Entry::Assistant(_)
        | Entry::Reasoning(_)
        | Entry::User(_)
        | Entry::Tool(_)
        | Entry::Notice(_)
        | Entry::RuntimeInfo(_)
        | Entry::Changelog(_)
        | Entry::Error(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct CachedCodeBlock {
    pub(super) line: usize,
    pub(super) copy_columns: Range<usize>,
    pub(super) text: Arc<str>,
}

/// One history entry's rendered payload. Code-block lines and image rows are
/// relative to this entry's first transcript row so surgical resplices never
/// shift absolute metadata for later entries.
#[derive(Clone, Debug, Default)]
pub(super) struct CachedEntry {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) code_blocks: Vec<CachedCodeBlock>,
    image_placement: Option<RenderedImagePlacements>,
    pub(super) incremental: Option<IncrementalEntryCache>,
    depends_on_image_height: bool,
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

    /// Width or theme changes reflow or restyle every cached line.
    pub(super) fn requires_full_rebuild(self, previous: Self) -> bool {
        self.width != previous.width || self.theme_generation != previous.theme_generation
    }
}

#[derive(Default)]
pub(super) struct HistoryLineCache {
    settings: Option<HistoryRenderSettings>,
    /// First transcript index stored in [`Self::entries`]. Earlier entries stay
    /// unmeasured until scroll asks for them.
    measured_from: usize,
    /// Rendered payload for `transcript[measured_from..]`.
    entries: Vec<CachedEntry>,
    /// Prefix ranges derived from [`CachedEntry::lines`] lengths, starting at
    /// line 0 of the measured suffix.
    entry_ranges: Vec<Range<usize>>,
    /// Transcript index to rebuild from. Never points into the unmeasured prefix.
    dirty_from: Option<usize>,
    /// Transcript indices to re-render in place (height may change). Applied on
    /// the next `ensure_current` without rebuilding the history suffix when the
    /// cache is already warm — used by tool expand/collapse.
    resplice: Vec<usize>,
    appended_entry: Option<usize>,
    /// Absolute-line projection of every entry's code blocks. Mouse hover and
    /// click hit-testing read this on every pointer event, so the projection is
    /// rebuilt lazily after entry mutations instead of on each call.
    projected_code_blocks: Option<Vec<CachedCodeBlock>>,
    /// When set, the last entry is still being streamed and must not own a trailing blank.
    open_stream_tail: bool,
    /// Test-only: counts entry renders so soft settings updates can prove they
    /// skipped work on text-only transcripts.
    #[cfg(test)]
    entry_renders: u64,
}

impl HistoryLineCache {
    /// Drop measured rows and treat `len` as the next unmeasured transcript
    /// length. The next paint measures a suffix instead of wrapping 0..len.
    pub(super) fn mark_unmeasured(&mut self, len: usize) {
        self.measured_from = len;
        self.clear_rendered();
        self.dirty_from = None;
    }

    /// Earlier transcript entries exist but have no wrapped rows yet.
    pub(super) fn has_unmeasured_prefix(&self) -> bool {
        self.measured_from > 0
    }

    pub(super) fn invalidate_from(&mut self, index: usize) {
        self.appended_entry = None;
        // Fold pending surgical marks into the suffix rebuild so an earlier
        // resplice is not lost when a later invalidation starts. Marks at or
        // after `index` are already covered by rebuilding from `index`.
        let mut dirty = index;
        for &resplice_index in &self.resplice {
            dirty = dirty.min(resplice_index);
        }
        self.resplice.clear();
        // Unmeasured prefix has no cached rows; start at the measured suffix.
        dirty = dirty.max(self.measured_from);
        self.dirty_from = Some(self.dirty_from.map_or(dirty, |existing| {
            existing.min(dirty).max(self.measured_from)
        }));
    }

    /// Re-render these entries on the next paint without dropping the cached
    /// suffix after them. Falls back to [`Self::invalidate_from`] when the
    /// cache is cold or already suffix-dirty.
    ///
    /// Used when tool cards toggle expand/collapse (height changes, content of
    /// later entries does not).
    pub(super) fn resplice_entries(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.appended_entry = None;
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
        if let Some(last) = self.cached_transcript_end().checked_sub(1) {
            // Surgical: only the last entry's trailing blank changes.
            self.resplice_entries([last]);
        }
    }

    /// Mark the last entry as text-appended (streaming assistant or reasoning)
    /// so the next paint extends its rendered lines instead of re-rendering.
    pub(super) fn entry_appended(&mut self, index: usize) {
        let can_extend = index + 1 == self.cached_transcript_end()
            && self.dirty_from.is_none()
            && self.resplice.is_empty()
            && self
                .cache_index(index)
                .and_then(|cache_index| self.entries.get(cache_index))
                .is_some_and(|entry| entry.incremental.is_some());
        if can_extend {
            self.appended_entry = Some(index);
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
        self.total_lines()
    }

    /// Measure from the tail until `min_lines` rows exist or the transcript
    /// starts. Leaves earlier entries unmeasured.
    pub(super) fn ensure_suffix(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        min_lines: usize,
        image_resolver: EntryImageResolver<'_>,
    ) {
        self.ensure_current(entries, settings, image_resolver);
        if self.total_lines() >= min_lines || self.measured_from == 0 {
            return;
        }
        self.grow_prefix(
            entries,
            settings,
            min_lines.saturating_sub(self.total_lines()),
            image_resolver,
        );
    }

    /// Render earlier entries until `extra_lines` more rows exist or the
    /// transcript starts. Returns how many lines were prepended.
    pub(super) fn grow_prefix(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        extra_lines: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> usize {
        if extra_lines == 0 || self.measured_from == 0 {
            return 0;
        }
        self.ensure_current(entries, settings, image_resolver);
        let before = self.total_lines();
        let target = before.saturating_add(extra_lines);
        let mut prepended = Vec::new();
        let mut added_lines = 0usize;
        while self.measured_from > 0 && before.saturating_add(added_lines) < target {
            let Some(cached) =
                self.take_previous_unmeasured_entry(entries, settings, image_resolver)
            else {
                break;
            };
            added_lines = added_lines.saturating_add(cached.lines.len());
            prepended.push(cached);
        }
        if prepended.is_empty() {
            return 0;
        }
        prepended.reverse();
        prepended.append(&mut self.entries);
        self.entries = prepended;
        self.recompute_ranges();
        self.total_lines().saturating_sub(before)
    }

    /// Absolute-line code-block projection, sorted by line. Test helper for
    /// projection contents; production hit-testing uses [`Self::code_block_at_line`].
    #[cfg(test)]
    pub(super) fn code_blocks(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) -> &[CachedCodeBlock] {
        self.ensure_projected_code_blocks(entries, settings, image_resolver)
    }

    /// Code block whose header row sits at absolute transcript `line`.
    pub(super) fn code_block_at_line(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        line: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> Option<CachedCodeBlock> {
        let blocks = self.ensure_projected_code_blocks(entries, settings, image_resolver);
        let index = blocks
            .binary_search_by(|block| block.line.cmp(&line))
            .ok()?;
        Some(blocks[index].clone())
    }

    fn ensure_projected_code_blocks(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) -> &[CachedCodeBlock] {
        self.ensure_current(entries, settings, image_resolver);
        if self.projected_code_blocks.is_none() {
            self.projected_code_blocks = Some(self.project_code_blocks(entries));
        }
        self.projected_code_blocks.as_deref().unwrap_or(&[])
    }

    pub(super) fn entry_index_at_line(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        line: usize,
        image_resolver: EntryImageResolver<'_>,
    ) -> Option<usize> {
        self.ensure_current(entries, settings, image_resolver);
        // Ranges are contiguous and sorted by start; binary search scales with
        // long transcripts where linear scan showed up in mouse hit-testing.
        let index = self.entry_ranges.partition_point(|range| range.end <= line);
        self.entry_ranges
            .get(index)
            .filter(|range| range.contains(&line))
            .map(|_| index.saturating_add(self.measured_from))
    }

    /// Cached line span of transcript `index`. Valid only while the cache is
    /// current (call after [`Self::entry_index_at_line`] ensured it).
    ///
    /// `index` is a transcript index, the same space [`Self::entry_index_at_line`]
    /// returns. Unmeasured prefix entries have no range.
    pub(super) fn entry_line_range(&self, transcript_index: usize) -> Option<Range<usize>> {
        let cache_index = self.cache_index(transcript_index)?;
        self.entry_ranges.get(cache_index).cloned()
    }

    #[cfg(test)]
    pub(super) fn entry_render_count(&self) -> u64 {
        self.entry_renders
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
            .min(self.total_lines());
        if slice.start >= end {
            return;
        }

        // Walk only entries that intersect the viewport instead of slicing a
        // flat transcript buffer.
        let mut entry_index = self
            .entry_ranges
            .partition_point(|range| range.end <= slice.start);
        let mut line = slice.start;
        while line < end && entry_index < self.entries.len() {
            let range = &self.entry_ranges[entry_index];
            if range.start >= end {
                break;
            }
            let local_start = line.saturating_sub(range.start);
            let local_end = end.min(range.end).saturating_sub(range.start);
            target.extend(
                self.entries[entry_index].lines[local_start..local_end]
                    .iter()
                    .cloned(),
            );
            line = range.start.saturating_add(local_end);
            entry_index += 1;
        }
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
        let mut visible = Vec::new();
        // Placement rows live inside their entry's range, so only entries
        // intersecting the window can contribute; skip the rest of a long
        // transcript instead of scanning every entry per frame.
        let first = self
            .entry_ranges
            .partition_point(|range| range.end <= start);
        for (entry, range) in self.entries[first..]
            .iter()
            .zip(self.entry_ranges[first..].iter())
        {
            if range.start >= end {
                break;
            }
            let Some(placements) = &entry.image_placement else {
                continue;
            };
            for placement in placements.iter() {
                let abs_start = range.start.saturating_add(placement.rows.start);
                let abs_end = range.start.saturating_add(placement.rows.end);
                let visible_start = abs_start.max(start);
                let visible_end = abs_end.min(end);
                if visible_start < visible_end {
                    visible.push(super::feed_image::VisibleImagePlacement {
                        image: placement.image.clone(),
                        row: visible_start - start,
                        height: visible_end - visible_start,
                        total_height: abs_end - abs_start,
                        skip_rows: visible_start - abs_start,
                    });
                }
            }
        }
        visible
    }

    fn total_lines(&self) -> usize {
        self.entry_ranges.last().map_or(0, |range| range.end)
    }

    fn clear_rendered(&mut self) {
        self.entries.clear();
        self.entry_ranges.clear();
        self.appended_entry = None;
        self.resplice.clear();
        self.projected_code_blocks = None;
    }

    fn truncate_entries_to(&mut self, rebuild_from: usize) {
        self.entries.truncate(rebuild_from);
        self.entry_ranges.truncate(rebuild_from);
        self.projected_code_blocks = None;
    }

    /// Rebuild absolute ranges from per-entry line lengths (source of truth).
    fn recompute_ranges(&mut self) {
        self.projected_code_blocks = None;
        self.entry_ranges.clear();
        self.entry_ranges.reserve(self.entries.len());
        let mut start = 0usize;
        for entry in &self.entries {
            let end = start.saturating_add(entry.lines.len());
            self.entry_ranges.push(start..end);
            start = end;
        }
    }

    fn project_code_blocks(&self, transcript: &[Entry]) -> Vec<CachedCodeBlock> {
        let mut blocks = Vec::new();
        for ((entry, range), source) in self
            .entries
            .iter()
            .zip(&self.entry_ranges)
            .zip(&transcript[self.measured_from..])
        {
            blocks.extend(entry.code_blocks.iter().map(|block| {
                CachedCodeBlock {
                    line: range.start.saturating_add(block.line),
                    copy_columns: block.copy_columns.clone(),
                    text: entry
                        .incremental
                        .as_ref()
                        .and_then(|cache| {
                            incremental_entry_source(source)
                                .and_then(|(text, _)| cache.fence_copy_source(text, block.line))
                        })
                        .map_or_else(|| Arc::clone(&block.text), Arc::from),
                }
            }));
        }
        blocks
    }

    fn cached_transcript_end(&self) -> usize {
        self.measured_from.saturating_add(self.entries.len())
    }

    fn cache_index(&self, transcript_index: usize) -> Option<usize> {
        transcript_index.checked_sub(self.measured_from)
    }

    fn take_previous_unmeasured_entry(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) -> Option<CachedEntry> {
        let index = self.measured_from.checked_sub(1)?;
        let Some(entry) = entries.get(index) else {
            self.measured_from = 0;
            return None;
        };
        #[cfg(test)]
        {
            self.entry_renders = self.entry_renders.saturating_add(1);
        }
        let cached = cached_entry_from_render(
            prepare_cache_entry_render(
                entry,
                index,
                entries.len(),
                settings,
                self.open_stream_tail,
                image_resolver,
            ),
            entry,
            index + 1 == entries.len(),
            settings.width,
            self.open_stream_tail,
        );
        self.measured_from = index;
        Some(cached)
    }

    fn soft_resplice_indices(&self, delta: SoftSettingsDelta, entries: &[Entry]) -> Vec<usize> {
        let mut indices = Vec::new();
        if delta.image_height {
            indices.extend(
                self.entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        entry
                            .depends_on_image_height
                            .then_some(index.saturating_add(self.measured_from))
                    }),
            );
            if delta.image_only() {
                return indices;
            }
        }
        if delta.tool_output || delta.zen {
            for (index, entry) in entries.iter().enumerate().skip(self.measured_from) {
                if delta.needs_entry(entry) {
                    indices.push(index);
                }
            }
            indices.sort_unstable();
            indices.dedup();
        }
        indices
    }

    fn ensure_current(
        &mut self,
        entries: &[Entry],
        settings: HistoryRenderSettings,
        image_resolver: EntryImageResolver<'_>,
    ) {
        if self.settings != Some(settings) {
            let soft = self
                .settings
                .and_then(|previous| SoftSettingsDelta::between(previous, settings));
            if let Some(delta) = soft {
                // Soft knobs (image budget, tool collapse height, zen) only
                // touch discrete entries. Keep the warm suffix so long
                // transcripts do not re-markdown on every composer resize.
                self.settings = Some(settings);
                let indices = self.soft_resplice_indices(delta, entries);
                self.resplice_entries(indices);
            } else {
                self.settings = Some(settings);
                self.clear_rendered();
                self.dirty_from = Some(0);
            }
        }

        let cached_end = self.cached_transcript_end();
        match entries.len().cmp(&cached_end) {
            std::cmp::Ordering::Less => {
                if entries.len() < self.measured_from {
                    self.mark_unmeasured(entries.len());
                } else {
                    self.invalidate_from(entries.len());
                }
            }
            std::cmp::Ordering::Equal => {}
            std::cmp::Ordering::Greater => self.invalidate_from(cached_end),
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
        let rebuild_from = dirty_from.max(self.measured_from).min(entries.len());
        if self.appended_entry.take() == Some(rebuild_from)
            && self.try_extend_last_entry(entries, rebuild_from, settings.width)
        {
            return;
        }
        let cache_rebuild = rebuild_from.saturating_sub(self.measured_from);
        self.truncate_entries_to(cache_rebuild);

        for (entry_index, entry) in entries.iter().enumerate().skip(rebuild_from) {
            self.push_rendered_entry(entry_index, entry, entries.len(), settings, image_resolver);
        }
    }

    /// Re-render `indices` in place. Returns false when the cache cannot support
    /// a surgical update.
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
        if self.entries.len() != entries.len().saturating_sub(self.measured_from)
            || self.entry_ranges.len() != self.entries.len()
            || self.settings != Some(settings)
        {
            return false;
        }
        for &index in indices {
            if index >= entries.len() || self.cache_index(index).is_none() {
                return false;
            }
        }

        for &index in indices {
            let Some(cache_index) = self.cache_index(index) else {
                return false;
            };
            #[cfg(test)]
            {
                self.entry_renders = self.entry_renders.saturating_add(1);
            }
            let entry = &entries[index];
            self.entries[cache_index] = cached_entry_from_render(
                prepare_cache_entry_render(
                    entry,
                    index,
                    entries.len(),
                    settings,
                    self.open_stream_tail,
                    image_resolver,
                ),
                entry,
                index + 1 == entries.len(),
                settings.width,
                self.open_stream_tail,
            );
        }

        self.recompute_ranges();
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
        #[cfg(test)]
        {
            self.entry_renders = self.entry_renders.saturating_add(1);
        }
        self.projected_code_blocks = None;
        let range_start = self.total_lines();
        let cached = cached_entry_from_render(
            prepare_cache_entry_render(
                entry,
                entry_index,
                entries_len,
                settings,
                self.open_stream_tail,
                image_resolver,
            ),
            entry,
            entry_index + 1 == entries_len,
            settings.width,
            self.open_stream_tail,
        );
        let line_count = cached.lines.len();
        self.entries.push(cached);
        self.entry_ranges
            .push(range_start..range_start.saturating_add(line_count));
    }

    fn try_extend_last_entry(&mut self, entries: &[Entry], index: usize, width: usize) -> bool {
        let Some(cache_index) = self.cache_index(index) else {
            return false;
        };
        let Some((text, render)) = entries.get(index).and_then(incremental_entry_source) else {
            return false;
        };
        let Some(range) = self.entry_ranges.get(cache_index).cloned() else {
            return false;
        };
        let has_trailing_blank = !(self.open_stream_tail && index + 1 == entries.len());
        let content_len = range.end.saturating_sub(range.start);
        let content_end = if has_trailing_blank {
            content_len.saturating_sub(1)
        } else {
            content_len
        };
        {
            let Some(cached) = self.entries.get(cache_index) else {
                return false;
            };
            let Some(cache) = cached.incremental.as_ref() else {
                return false;
            };
            if cache.stable_source_len > text.len() {
                return false;
            }
            if cached.image_placement.as_ref().is_some_and(|placements| {
                placements
                    .iter()
                    .any(|placement| placement.rows.start < content_end && 0 < placement.rows.end)
            }) {
                return false;
            }
            let mutable_source = &text[cache.stable_source_len..];
            if !cache.remains_inside_fence(text)
                && !super::markdown_image::collect_markdown_image_sources(mutable_source).is_empty()
            {
                return false;
            }
        }

        let reasoning = matches!(entries.get(index), Some(Entry::Reasoning(_)));
        if incremental::extend_last_entry(
            &mut self.entries[cache_index],
            text,
            render,
            width,
            has_trailing_blank,
            content_end,
            reasoning,
        ) {
            debug_assert_eq!(cache_index + 1, self.entries.len());
            debug_assert_eq!(self.entry_ranges.len(), self.entries.len());
            self.entry_ranges[cache_index].end = range
                .start
                .saturating_add(self.entries[cache_index].lines.len());
            self.projected_code_blocks = None;
            return true;
        }
        false
    }
}

pub(super) fn append_entry_segment_into(
    lines: &mut Vec<Line<'static>>,
    code_blocks: &mut Vec<CachedCodeBlock>,
    text: &str,
    width: usize,
    render: EntryContentRender,
) {
    if text.is_empty() {
        return;
    }
    let local_start = lines.len();
    let rendered = render(text, width);
    code_blocks.extend(
        rendered
            .code_blocks
            .into_iter()
            .map(|block| CachedCodeBlock {
                line: local_start + block.top_line,
                copy_columns: block.copy_columns.start.saturating_add(1)
                    ..block.copy_columns.end.saturating_add(1),
                text: Arc::from(block.text),
            }),
    );
    lines.extend(rendered.lines.into_iter().map(pad_display_line));
}

/// Shared entry render for full rebuild and surgical resplice paths.
///
/// Returns `None` for hidden entries. Code-block line numbers and image
/// placements are relative to the entry start.
struct PreparedCacheEntry {
    lines: Vec<Line<'static>>,
    code_blocks: Vec<CachedCodeBlock>,
    image_placement: Option<RenderedImagePlacements>,
    depends_on_image_height: bool,
}

fn cached_entry_from_render(
    prepared: Option<PreparedCacheEntry>,
    entry: &Entry,
    is_last: bool,
    width: usize,
    open_stream_tail: bool,
) -> CachedEntry {
    let Some(rendered) = prepared else {
        return CachedEntry::default();
    };
    let has_trailing_blank = !(open_stream_tail && is_last);
    let content_line_count = rendered
        .lines
        .len()
        .saturating_sub(usize::from(has_trailing_blank));
    CachedEntry {
        lines: rendered.lines,
        code_blocks: rendered.code_blocks,
        image_placement: rendered.image_placement,
        incremental: incremental::incremental_cache_for(entry, is_last, width, content_line_count),
        depends_on_image_height: rendered.depends_on_image_height,
    }
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
    // Height only moves with the budget when a real placement (or tool image)
    // is reserved. Unloaded markdown placeholders stay one row tall.
    let depends_on_image_height = match entry {
        Entry::Tool(tool) => tool.image.is_some(),
        Entry::User(_)
        | Entry::Assistant(_)
        | Entry::Reasoning(_)
        | Entry::Notice(_)
        | Entry::RuntimeInfo(_)
        | Entry::Changelog(_)
        | Entry::Error(_) => rendered.image_placement.is_some(),
    };
    let code_blocks = rendered
        .code_blocks
        .into_iter()
        .map(|block| CachedCodeBlock {
            // Relative to entry start; absolute projection happens on read.
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
        depends_on_image_height,
    })
}

#[cfg(test)]
#[path = "../history_cache_tests.rs"]
mod tests;
