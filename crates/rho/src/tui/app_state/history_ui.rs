//! Transcript history, scroll, selection, and related render caches.

use std::time::{Duration, Instant};

use crate::tui::{
    history_cache::HistoryLineCache,
    markdown_image,
    scrollbar::HistoryScrollbarDrag,
    text_selection::{CopyNotice, TextSelection},
    Entry, HistoryScroll, SessionHeaderCache,
};

/// Transcript history, scroll, selection, and related render caches.
///
/// Fields stay private so transcript and cache updates go through methods that
/// keep line/image invalidation consistent.
#[derive(Default)]
pub(in crate::tui) struct HistoryUi {
    transcript: Vec<Entry>,
    lines: HistoryLineCache,
    last_status_notice: Option<String>,
    last_inserted_was_tool: bool,
    images: markdown_image::MarkdownImageCache,
    images_dirty_from: Option<usize>,
    scroll: HistoryScroll,
    scrollbar_drag: Option<HistoryScrollbarDrag>,
    scrollbar_visible_until: Option<Instant>,
    scrollbar_hovered: bool,
    hovered_code_block_copy: Option<usize>,
    text_selection: Option<TextSelection>,
    copy_notice: Option<CopyNotice>,
    session_header_cache: Option<SessionHeaderCache>,
}

impl HistoryUi {
    pub(in crate::tui) fn entries(&self) -> &[Entry] {
        &self.transcript
    }

    pub(in crate::tui) fn entries_mut(&mut self) -> &mut Vec<Entry> {
        &mut self.transcript
    }

    pub(in crate::tui) fn len(&self) -> usize {
        self.transcript.len()
    }

    pub(in crate::tui) fn last(&self) -> Option<&Entry> {
        self.transcript.last()
    }

    pub(in crate::tui) fn last_mut(&mut self) -> Option<&mut Entry> {
        self.transcript.last_mut()
    }

    pub(in crate::tui) fn clear_entries(&mut self) {
        self.transcript.clear();
    }

    pub(in crate::tui) fn set_entries(&mut self, entries: Vec<Entry>) {
        self.transcript = entries;
    }

    pub(in crate::tui) fn push(&mut self, entry: Entry) {
        self.transcript.push(entry);
    }

    pub(in crate::tui) fn get(&self, index: usize) -> Option<&Entry> {
        self.transcript.get(index)
    }

    pub(in crate::tui) fn get_mut(&mut self, index: usize) -> Option<&mut Entry> {
        self.transcript.get_mut(index)
    }

    pub(in crate::tui) fn lines_mut(&mut self) -> &mut HistoryLineCache {
        &mut self.lines
    }

    /// Borrow lines mutably with entries and images for cache updates.
    pub(in crate::tui) fn with_lines_and_images_mut<R>(
        &mut self,
        f: impl FnOnce(&mut HistoryLineCache, &[Entry], &markdown_image::MarkdownImageCache) -> R,
    ) -> R {
        f(&mut self.lines, &self.transcript, &self.images)
    }

    /// Drop cached history lines and mark markdown images dirty from `index`.
    pub(in crate::tui) fn invalidate_from(&mut self, index: usize) {
        self.lines.invalidate_from(index);
        self.images_dirty_from = Some(
            self.images_dirty_from
                .map_or(index, |dirty_from| dirty_from.min(index)),
        );
    }

    pub(in crate::tui) fn scroll(&self) -> HistoryScroll {
        self.scroll
    }

    pub(in crate::tui) fn set_scroll(&mut self, scroll: HistoryScroll) {
        self.scroll = scroll;
    }

    pub(in crate::tui) fn scroll_to_bottom(&mut self) {
        self.scroll = HistoryScroll::Bottom;
        self.hide_scrollbar();
    }

    pub(in crate::tui) fn scrollbar_drag(&self) -> Option<HistoryScrollbarDrag> {
        self.scrollbar_drag
    }

    pub(in crate::tui) fn set_scrollbar_drag(&mut self, drag: Option<HistoryScrollbarDrag>) {
        self.scrollbar_drag = drag;
    }

    pub(in crate::tui) fn scrollbar_visible_until(&self) -> Option<Instant> {
        self.scrollbar_visible_until
    }

    pub(in crate::tui) fn reveal_scrollbar(&mut self, now: Instant, duration: Duration) {
        self.scrollbar_visible_until = Some(now + duration);
    }

    pub(in crate::tui) fn hide_scrollbar(&mut self) {
        self.scrollbar_drag = None;
        self.scrollbar_visible_until = None;
        self.scrollbar_hovered = false;
    }

    pub(in crate::tui) fn scrollbar_hovered(&self) -> bool {
        self.scrollbar_hovered
    }

    pub(in crate::tui) fn set_scrollbar_hovered(&mut self, hovered: bool) {
        self.scrollbar_hovered = hovered;
    }

    pub(in crate::tui) fn should_render_scrollbar(&self, now: Instant) -> bool {
        self.scrollbar_drag.is_some()
            || self.scrollbar_hovered
            || self
                .scrollbar_visible_until
                .is_some_and(|visible_until| now < visible_until)
    }

    pub(in crate::tui) fn text_selection(&self) -> Option<TextSelection> {
        self.text_selection
    }

    pub(in crate::tui) fn text_selection_mut(&mut self) -> &mut Option<TextSelection> {
        &mut self.text_selection
    }

    pub(in crate::tui) fn clear_text_selection(&mut self) {
        self.text_selection = None;
    }

    pub(in crate::tui) fn copy_notice(&self) -> Option<&CopyNotice> {
        self.copy_notice.as_ref()
    }

    pub(in crate::tui) fn set_copy_notice(&mut self, notice: Option<CopyNotice>) {
        self.copy_notice = notice;
    }

    pub(in crate::tui) fn images(&self) -> &markdown_image::MarkdownImageCache {
        &self.images
    }

    pub(in crate::tui) fn images_mut(&mut self) -> &mut markdown_image::MarkdownImageCache {
        &mut self.images
    }

    pub(in crate::tui) fn images_dirty_from(&self) -> Option<usize> {
        self.images_dirty_from
    }

    pub(in crate::tui) fn take_images_dirty_from(&mut self) -> Option<usize> {
        self.images_dirty_from.take()
    }

    pub(in crate::tui) fn set_images_dirty_from(&mut self, index: Option<usize>) {
        self.images_dirty_from = index;
    }

    pub(in crate::tui) fn last_status_notice(&self) -> Option<&str> {
        self.last_status_notice.as_deref()
    }

    pub(in crate::tui) fn set_last_status_notice(&mut self, notice: Option<String>) {
        self.last_status_notice = notice;
    }

    pub(in crate::tui) fn last_inserted_was_tool(&self) -> bool {
        self.last_inserted_was_tool
    }

    pub(in crate::tui) fn set_last_inserted_was_tool(&mut self, value: bool) {
        self.last_inserted_was_tool = value;
    }

    pub(in crate::tui) fn hovered_code_block_copy(&self) -> Option<usize> {
        self.hovered_code_block_copy
    }

    pub(in crate::tui) fn set_hovered_code_block_copy(&mut self, line: Option<usize>) {
        self.hovered_code_block_copy = line;
    }

    pub(in crate::tui) fn session_header_cache(&self) -> Option<&SessionHeaderCache> {
        self.session_header_cache.as_ref()
    }

    pub(in crate::tui) fn set_session_header_cache(&mut self, cache: Option<SessionHeaderCache>) {
        self.session_header_cache = cache;
    }
}
