use std::{ops::Range, sync::Arc};

use ratatui::text::Line;

use super::{
    feed_image::{FeedImage, RenderedImagePlacement, RenderedImagePlacements},
    history_soft_settings::SoftSettingsDelta,
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

    /// Width or theme changes reflow or restyle every cached line.
    pub(super) fn requires_full_rebuild(self, previous: Self) -> bool {
        self.width != previous.width || self.theme_generation != previous.theme_generation
    }
}

#[derive(Default)]
pub(super) struct HistoryLineCache {
    settings: Option<HistoryRenderSettings>,
    /// Per-entry rendered lines. Keeps suffix resplices from shifting a giant
    /// flat buffer; visible paint walks only the entries that intersect the
    /// viewport.
    entry_lines: Vec<Arc<[Line<'static>]>>,
    /// Prefix ranges derived from [`Self::entry_lines`] lengths. Rebuilt after
    /// surgical height changes so absolute offsets stay coherent.
    entry_ranges: Vec<Range<usize>>,
    assistant_caches: Vec<Option<IncrementalAssistantCache>>,
    /// Sorted entry indices whose height depends on
    /// [`HistoryRenderSettings::max_image_height`].
    image_height_dep_entries: Vec<usize>,
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
    /// Test-only: counts entry renders so soft settings updates can prove they
    /// skipped work on text-only transcripts.
    #[cfg(test)]
    entry_renders: u64,
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
        self.total_lines()
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
        // Ranges are contiguous and sorted by start; binary search scales with
        // long transcripts where linear scan showed up in mouse hit-testing.
        let index = self.entry_ranges.partition_point(|range| range.end <= line);
        self.entry_ranges
            .get(index)
            .filter(|range| range.contains(&line))
            .map(|_| index)
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
        while line < end && entry_index < self.entry_lines.len() {
            let range = &self.entry_ranges[entry_index];
            if range.start >= end {
                break;
            }
            let local_start = line.saturating_sub(range.start);
            let local_end = end.min(range.end).saturating_sub(range.start);
            target.extend(
                self.entry_lines[entry_index][local_start..local_end]
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

    fn total_lines(&self) -> usize {
        self.entry_ranges.last().map_or(0, |range| range.end)
    }

    fn clear_rendered(&mut self) {
        self.entry_lines.clear();
        self.entry_ranges.clear();
        self.assistant_caches.clear();
        self.image_height_dep_entries.clear();
        self.code_blocks.clear();
        self.image_placements.clear();
        self.appended_assistant = None;
        self.resplice.clear();
    }

    fn set_image_height_dep(&mut self, index: usize, depends: bool) {
        match self.image_height_dep_entries.binary_search(&index) {
            Ok(pos) if !depends => {
                self.image_height_dep_entries.remove(pos);
            }
            Err(pos) if depends => {
                self.image_height_dep_entries.insert(pos, index);
            }
            _ => {}
        }
    }

    fn truncate_entries_to(&mut self, rebuild_from: usize) {
        self.entry_lines.truncate(rebuild_from);
        self.entry_ranges.truncate(rebuild_from);
        self.assistant_caches.truncate(rebuild_from);
        self.image_height_dep_entries
            .retain(|&index| index < rebuild_from);
        let line_start = self.total_lines();
        self.code_blocks.retain(|block| block.line < line_start);
        self.image_placements = self
            .image_placements
            .iter()
            .filter_map(|placements| placements.retain_starting_before(line_start))
            .collect();
    }

    /// Rebuild absolute ranges from per-entry line lengths (source of truth).
    fn recompute_ranges(&mut self) {
        self.entry_ranges.clear();
        self.entry_ranges.reserve(self.entry_lines.len());
        let mut start = 0usize;
        for lines in &self.entry_lines {
            let end = start.saturating_add(lines.len());
            self.entry_ranges.push(start..end);
            start = end;
        }
    }

    fn schedule_soft_resplice(&mut self, indices: Vec<usize>) {
        if indices.is_empty() {
            return;
        }
        if let Some(dirty) = self.dirty_from {
            let min = indices.iter().copied().min().unwrap_or(dirty);
            self.dirty_from = Some(dirty.min(min));
        } else {
            self.resplice_entries(indices);
        }
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
                let indices = delta.resplice_indices(entries, &self.image_height_dep_entries);
                self.schedule_soft_resplice(indices);
            } else {
                self.settings = Some(settings);
                self.clear_rendered();
                self.dirty_from = Some(0);
            }
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
        self.truncate_entries_to(rebuild_from);

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
            || self.entry_lines.len() != entries.len()
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
            #[cfg(test)]
            {
                self.entry_renders = self.entry_renders.saturating_add(1);
            }
            let old_range = self.entry_ranges[index].clone();
            let entry = &entries[index];
            let prepared = prepare_cache_entry_render(
                entry,
                index,
                entries.len(),
                settings,
                self.open_stream_tail,
                image_resolver,
            );
            let (new_lines, new_code_blocks, new_image, depends_on_image_height) = match prepared {
                None => (Vec::new(), Vec::new(), None, false),
                Some(rendered) => (
                    rendered.lines,
                    rendered.code_blocks,
                    rendered.image_placement,
                    rendered.depends_on_image_height,
                ),
            };

            let start = old_range.start;
            let end = old_range.end;
            let old_len = end - start;
            let new_len = new_lines.len();
            let delta = new_len as isize - old_len as isize;

            self.entry_lines[index] = Arc::from(new_lines);
            self.set_image_height_dep(index, depends_on_image_height);

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

        self.recompute_ranges();
        if !self.ranges_match_entry_lines() {
            // Should be unreachable if lengths stayed coherent; force a full
            // rebuild rather than paint a torn cache.
            self.clear_rendered();
            self.dirty_from = Some(0);
            return false;
        }
        true
    }

    fn ranges_match_entry_lines(&self) -> bool {
        if self.entry_ranges.len() != self.entry_lines.len() {
            return false;
        }
        let mut start = 0usize;
        for (range, lines) in self.entry_ranges.iter().zip(self.entry_lines.iter()) {
            let end = start.saturating_add(lines.len());
            if *range != (start..end) {
                return false;
            }
            start = end;
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
        #[cfg(test)]
        {
            self.entry_renders = self.entry_renders.saturating_add(1);
        }
        let range_start = self.total_lines();
        let Some(rendered) = prepare_cache_entry_render(
            entry,
            entry_index,
            entries_len,
            settings,
            self.open_stream_tail,
            image_resolver,
        ) else {
            // Keep a zero-height range so entry indices stay aligned with history.
            self.entry_lines
                .push(Arc::<[Line<'static>]>::from(Vec::new()));
            self.entry_ranges.push(range_start..range_start);
            self.assistant_caches.push(None);
            self.set_image_height_dep(entry_index, false);
            return;
        };

        let entry_start = range_start;
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
        let line_count = rendered.lines.len();
        self.entry_lines.push(Arc::from(rendered.lines));
        self.entry_ranges
            .push(range_start..range_start.saturating_add(line_count));
        self.set_image_height_dep(entry_index, rendered.depends_on_image_height);
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
        let content_len = range.end.saturating_sub(range.start);
        let content_end = if has_trailing_blank {
            content_len.saturating_sub(1)
        } else {
            content_len
        };
        // Content starts at range.start; there is no leading spacer.
        let preserve_end = cache.stable_line_count;
        if preserve_end >= content_end || preserve_end > self.entry_lines[index].len() {
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

        let mut lines = self.entry_lines[index][..preserve_end].to_vec();
        let trailing_blank =
            has_trailing_blank.then(|| self.entry_lines[index][content_len - 1].clone());
        let absolute_preserve_end = range.start.saturating_add(preserve_end);
        self.code_blocks
            .retain(|block| block.line < absolute_preserve_end);

        let previous_stable_source_len = cache.stable_source_len;
        append_assistant_segment_into(
            &mut lines,
            &mut self.code_blocks,
            range.start,
            &text[previous_stable_source_len..new_tail_start],
            width,
        );
        let cache = self.assistant_caches[index]
            .as_mut()
            .expect("assistant cache exists");
        cache.stable_line_count = lines.len();
        cache.stable_source_len = new_tail_start;
        append_assistant_segment_into(
            &mut lines,
            &mut self.code_blocks,
            range.start,
            &text[new_tail_start..],
            width,
        );
        if let Some(trailing_blank) = trailing_blank {
            lines.push(trailing_blank);
        }
        self.entry_lines[index] = Arc::from(lines);
        self.recompute_ranges();
        true
    }
}

fn offset_usize(value: usize, delta: isize) -> usize {
    if delta >= 0 {
        value.saturating_add(delta as usize)
    } else {
        value.saturating_sub((-delta) as usize)
    }
}

fn append_assistant_segment_into(
    lines: &mut Vec<Line<'static>>,
    code_blocks: &mut Vec<CachedCodeBlock>,
    entry_line_start: usize,
    text: &str,
    width: usize,
) {
    if text.is_empty() {
        return;
    }
    let local_start = lines.len();
    let rendered = render_assistant_content(text, width);
    code_blocks.extend(
        rendered
            .code_blocks
            .into_iter()
            .map(|block| CachedCodeBlock {
                line: entry_line_start + local_start + block.top_line,
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
/// placements are relative to the entry start; callers relocate them.
struct PreparedCacheEntry {
    lines: Vec<Line<'static>>,
    code_blocks: Vec<CachedCodeBlock>,
    image_placement: Option<RenderedImagePlacements>,
    depends_on_image_height: bool,
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
        _ => rendered.image_placement.is_some(),
    };
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
        depends_on_image_height,
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
