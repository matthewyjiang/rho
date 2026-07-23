//! Cohesive App-owned UI state groups for history, composer input, and pending work.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rho_providers::model::ImageContent;

use super::{
    history_cache::HistoryLineCache,
    inline_shell::InlineShellMode,
    markdown_image,
    paste_burst::{expand_paste_segments, PasteBurst},
    pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel},
    scrollbar::HistoryScrollbarDrag,
    text_selection::{CopyNotice, TextSelection},
    ComposerMode, Entry, FileMatchCache, HistoryScroll, InputDraft, InputSubmissionMode,
    PasteSegment, QueuedPrompt, SessionHeaderCache, SkillMatchCache,
};

/// Transcript history, scroll, selection, and related render caches.
///
/// Fields stay private so transcript and cache updates go through methods that
/// keep line/image invalidation consistent.
#[derive(Default)]
pub(super) struct HistoryUi {
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
    pub(super) fn entries(&self) -> &[Entry] {
        &self.transcript
    }

    pub(super) fn entries_mut(&mut self) -> &mut Vec<Entry> {
        &mut self.transcript
    }

    pub(super) fn len(&self) -> usize {
        self.transcript.len()
    }

    pub(super) fn last(&self) -> Option<&Entry> {
        self.transcript.last()
    }

    pub(super) fn last_mut(&mut self) -> Option<&mut Entry> {
        self.transcript.last_mut()
    }

    pub(super) fn clear_entries(&mut self) {
        self.transcript.clear();
    }

    pub(super) fn set_entries(&mut self, entries: Vec<Entry>) {
        self.transcript = entries;
    }

    pub(super) fn push(&mut self, entry: Entry) {
        self.transcript.push(entry);
    }

    pub(super) fn get(&self, index: usize) -> Option<&Entry> {
        self.transcript.get(index)
    }

    pub(super) fn get_mut(&mut self, index: usize) -> Option<&mut Entry> {
        self.transcript.get_mut(index)
    }

    pub(super) fn lines_mut(&mut self) -> &mut HistoryLineCache {
        &mut self.lines
    }

    /// Borrow lines mutably with entries and images for cache updates.
    pub(super) fn with_lines_and_images_mut<R>(
        &mut self,
        f: impl FnOnce(&mut HistoryLineCache, &[Entry], &markdown_image::MarkdownImageCache) -> R,
    ) -> R {
        f(&mut self.lines, &self.transcript, &self.images)
    }

    /// Drop cached history lines and mark markdown images dirty from `index`.
    pub(super) fn invalidate_from(&mut self, index: usize) {
        self.lines.invalidate_from(index);
        self.images_dirty_from = Some(
            self.images_dirty_from
                .map_or(index, |dirty_from| dirty_from.min(index)),
        );
    }

    pub(super) fn scroll(&self) -> HistoryScroll {
        self.scroll
    }

    pub(super) fn set_scroll(&mut self, scroll: HistoryScroll) {
        self.scroll = scroll;
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll = HistoryScroll::Bottom;
        self.hide_scrollbar();
    }

    pub(super) fn scrollbar_drag(&self) -> Option<HistoryScrollbarDrag> {
        self.scrollbar_drag
    }

    pub(super) fn set_scrollbar_drag(&mut self, drag: Option<HistoryScrollbarDrag>) {
        self.scrollbar_drag = drag;
    }

    pub(super) fn scrollbar_visible_until(&self) -> Option<Instant> {
        self.scrollbar_visible_until
    }

    pub(super) fn reveal_scrollbar(&mut self, now: Instant, duration: Duration) {
        self.scrollbar_visible_until = Some(now + duration);
    }

    pub(super) fn hide_scrollbar(&mut self) {
        self.scrollbar_drag = None;
        self.scrollbar_visible_until = None;
        self.scrollbar_hovered = false;
    }

    pub(super) fn scrollbar_hovered(&self) -> bool {
        self.scrollbar_hovered
    }

    pub(super) fn set_scrollbar_hovered(&mut self, hovered: bool) {
        self.scrollbar_hovered = hovered;
    }

    pub(super) fn should_render_scrollbar(&self, now: Instant) -> bool {
        self.scrollbar_drag.is_some()
            || self.scrollbar_hovered
            || self
                .scrollbar_visible_until
                .is_some_and(|visible_until| now < visible_until)
    }

    pub(super) fn text_selection(&self) -> Option<TextSelection> {
        self.text_selection
    }

    pub(super) fn text_selection_mut(&mut self) -> &mut Option<TextSelection> {
        &mut self.text_selection
    }

    pub(super) fn clear_text_selection(&mut self) {
        self.text_selection = None;
    }

    pub(super) fn copy_notice(&self) -> Option<&CopyNotice> {
        self.copy_notice.as_ref()
    }

    pub(super) fn set_copy_notice(&mut self, notice: Option<CopyNotice>) {
        self.copy_notice = notice;
    }

    pub(super) fn images(&self) -> &markdown_image::MarkdownImageCache {
        &self.images
    }

    pub(super) fn images_mut(&mut self) -> &mut markdown_image::MarkdownImageCache {
        &mut self.images
    }

    pub(super) fn images_dirty_from(&self) -> Option<usize> {
        self.images_dirty_from
    }

    pub(super) fn take_images_dirty_from(&mut self) -> Option<usize> {
        self.images_dirty_from.take()
    }

    pub(super) fn set_images_dirty_from(&mut self, index: Option<usize>) {
        self.images_dirty_from = index;
    }

    pub(super) fn last_status_notice(&self) -> Option<&str> {
        self.last_status_notice.as_deref()
    }

    pub(super) fn set_last_status_notice(&mut self, notice: Option<String>) {
        self.last_status_notice = notice;
    }

    pub(super) fn last_inserted_was_tool(&self) -> bool {
        self.last_inserted_was_tool
    }

    pub(super) fn set_last_inserted_was_tool(&mut self, value: bool) {
        self.last_inserted_was_tool = value;
    }

    pub(super) fn hovered_code_block_copy(&self) -> Option<usize> {
        self.hovered_code_block_copy
    }

    pub(super) fn set_hovered_code_block_copy(&mut self, line: Option<usize>) {
        self.hovered_code_block_copy = line;
    }

    pub(super) fn session_header_cache(&self) -> Option<&SessionHeaderCache> {
        self.session_header_cache.as_ref()
    }

    pub(super) fn set_session_header_cache(&mut self, cache: Option<SessionHeaderCache>) {
        self.session_header_cache = cache;
    }
}

/// Composer text, paste handling, command/file palettes, and input history.
#[derive(Default)]
pub(super) struct InputUi {
    pub(in crate::tui) text: String,
    pub(in crate::tui) cursor: usize,
    pub(in crate::tui) shell_mode: Option<InlineShellMode>,
    pub(in crate::tui) pending_images: Vec<ImageContent>,
    pub(in crate::tui) history: Vec<String>,
    pub(in crate::tui) history_cursor: Option<usize>,
    pub(in crate::tui) history_draft: Option<InputDraft>,
    pub(in crate::tui) paste_burst: PasteBurst,
    pub(in crate::tui) paste_segments: Vec<PasteSegment>,
    pub(in crate::tui) submission_mode: InputSubmissionMode,
    pub(in crate::tui) command_selection: usize,
    pub(in crate::tui) command_prefix: Option<String>,
    pub(in crate::tui) command_palette_dismissed: bool,
    pub(in crate::tui) file_selection: usize,
    pub(in crate::tui) file_query: Option<String>,
    pub(in crate::tui) file_palette_dismissed: bool,
    pub(in crate::tui) file_match_cache: Option<FileMatchCache>,
    pub(in crate::tui) skill_match_cache: Option<SkillMatchCache>,
    pub(in crate::tui) composer: ComposerMode,
}

impl InputUi {
    /// Clear composer text state after a successful submit.
    pub(super) fn clear_submitted(&mut self) {
        self.text.clear();
        self.paste_segments.clear();
        self.shell_mode = None;
        self.cursor = 0;
        self.pending_images.clear();
    }

    pub(super) fn expanded_text(&self) -> String {
        expand_paste_segments(&self.text, &self.paste_segments)
    }

    pub(super) fn reset_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(super) fn set_text_and_cursor(&mut self, text: impl Into<String>, cursor: usize) {
        self.text = text.into();
        self.cursor = cursor;
    }
}

/// Queued prompts, steering, and the pending-input panel.
#[derive(Default)]
pub(super) struct PendingWorkUi {
    pub(in crate::tui) steering_prompts: VecDeque<QueuedPrompt>,
    pub(in crate::tui) accepted_steering: VecDeque<AcceptedSteering>,
    pub(in crate::tui) retracting_steering: Option<rho_sdk::SteeringId>,
    pub(in crate::tui) input_panel: PendingInputPanel,
    pub(in crate::tui) input_action: Option<PendingInputAction>,
    pub(in crate::tui) queued_prompts: VecDeque<QueuedPrompt>,
}

impl PendingWorkUi {
    pub(super) fn follow_up_len(&self) -> usize {
        self.queued_prompts.len()
    }

    pub(super) fn has_follow_ups(&self) -> bool {
        !self.queued_prompts.is_empty()
    }

    /// Fold unapplied accepted and local steering into follow-up queue.
    pub(super) fn preserve_unapplied_steering_as_follow_ups(&mut self) {
        let mut pending = self
            .accepted_steering
            .drain(..)
            .map(|entry| entry.prompt)
            .chain(self.steering_prompts.drain(..))
            .collect::<VecDeque<_>>();
        pending.append(&mut self.queued_prompts);
        self.queued_prompts = pending;
        self.retracting_steering = None;
        self.input_action = None;
    }
}
