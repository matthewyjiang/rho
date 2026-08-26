//! Composer text, paste handling, command/file palettes, and input history.

use std::time::{Duration, Instant};

use crate::tui::{
    composer_attachments::ComposerAttachmentSlot,
    feed_image::FeedImage,
    inline_shell::InlineShellMode,
    paste_burst::{expand_paste_segments, PasteBurst},
    ChatMedia, ComposerAttachment, ComposerMode, InputDraft, InputSubmissionMode, MediaAttachId,
    PasteSegment, PendingAttachmentSource,
};

#[derive(Debug)]
pub(in crate::tui) struct AttachmentsPending;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerSelection {
    Characters {
        anchor: usize,
        focus: usize,
    },
    Range {
        start: usize,
        end: usize,
        focus: usize,
    },
}

impl ComposerSelection {
    fn characters(position: usize) -> Self {
        Self::Characters {
            anchor: position,
            focus: position,
        }
    }

    fn range(start: usize, end: usize) -> Self {
        Self::Range {
            start,
            end,
            focus: end,
        }
    }

    fn update(&mut self, position: usize) {
        match self {
            Self::Characters { focus, .. } | Self::Range { focus, .. } => *focus = position,
        }
    }

    fn pointer_origin(self) -> usize {
        match self {
            Self::Characters { anchor, .. } => anchor,
            Self::Range { start, .. } => start,
        }
    }

    fn focus(self) -> usize {
        match self {
            Self::Characters { focus, .. } => focus,
            Self::Range {
                start, end, focus, ..
            } => {
                if focus < start || focus > end {
                    focus
                } else {
                    end
                }
            }
        }
    }

    /// Ordered half-open char range when the selection spans text.
    fn edit_range(self) -> Option<std::ops::Range<usize>> {
        match self {
            Self::Characters { anchor, focus } if anchor < focus => Some(anchor..focus),
            Self::Characters { anchor, focus } if focus < anchor => Some(focus..anchor),
            Self::Characters { .. } => None,
            Self::Range {
                start, end, focus, ..
            } if focus < start => Some(focus..end),
            Self::Range {
                start, end, focus, ..
            } if focus > end => Some(start..focus),
            Self::Range { start, end, .. } if start < end => Some(start..end),
            Self::Range { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ComposerSelectionState {
    #[default]
    None,
    Dragging(ComposerSelection),
    Selected(ComposerSelection),
}

#[derive(Clone, Copy, Debug)]
struct ComposerClick {
    at: Instant,
    column: u16,
    row: u16,
    index: usize,
}

impl ComposerSelectionState {
    fn value(self) -> Option<ComposerSelection> {
        match self {
            Self::Dragging(selection) | Self::Selected(selection) => Some(selection),
            Self::None => None,
        }
    }
}

/// Composer text, paste handling, command/file palettes, and input history.
#[derive(Default)]
pub(in crate::tui) struct InputUi {
    text: String,
    cursor: usize,
    selection: ComposerSelectionState,
    last_pointer_click: Option<ComposerClick>,
    composer_view_start: usize,
    shell_mode: Option<InlineShellMode>,
    attachments: Vec<ComposerAttachmentSlot>,
    /// Bumped on every attachment mutation so composer layout caches invalidate.
    attachment_epoch: u64,
    history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: Option<InputDraft>,
    paste_burst: PasteBurst,
    paste_segments: Vec<PasteSegment>,
    submission_mode: InputSubmissionMode,
    command_selection: usize,
    command_prefix: Option<String>,
    command_palette_dismissed: bool,
    file_selection: usize,
    file_query: Option<String>,
    file_palette_dismissed: bool,
    composer: ComposerMode,
}

impl InputUi {
    /// Clear composer text state after a successful submit.
    pub(in crate::tui) fn clear_submitted(&mut self) {
        self.text.clear();
        self.paste_segments.clear();
        self.shell_mode = None;
        self.cursor = 0;
        self.selection = ComposerSelectionState::None;
        self.last_pointer_click = None;
        self.composer_view_start = 0;
        self.attachments.clear();
        self.attachment_epoch = self.attachment_epoch.wrapping_add(1);
    }

    pub(in crate::tui) fn expanded_text(&self) -> String {
        expand_paste_segments(&self.text, &self.paste_segments)
    }

    pub(in crate::tui) fn has_pending_draft(&self) -> bool {
        !self.text.is_empty()
            || self.shell_mode.is_some()
            || !self.attachments.is_empty()
            || self.paste_burst.has_pending()
    }

    pub(in crate::tui) fn reset_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(in crate::tui) fn set_text_and_cursor(&mut self, text: String, cursor: usize) {
        self.text = text;
        self.cursor = cursor;
        self.selection = ComposerSelectionState::None;
        self.last_pointer_click = None;
        self.composer_view_start = 0;
    }

    pub(in crate::tui) fn apply_input_draft(&mut self, draft: InputDraft) {
        self.shell_mode = draft.shell_mode;
        self.text = draft.input;
        self.paste_segments = draft.paste_segments;
        self.submission_mode = draft.submission_mode;
        self.cursor = self.text.chars().count();
        self.selection = ComposerSelectionState::None;
        self.last_pointer_click = None;
        self.composer_view_start = 0;
    }

    pub(in crate::tui) fn text(&self) -> &str {
        &self.text
    }

    /// Mutate composer text in place for insert/delete surgery.
    pub(in crate::tui) fn with_text_mut<R>(&mut self, f: impl FnOnce(&mut String) -> R) -> R {
        f(&mut self.text)
    }

    pub(in crate::tui) fn set_text(&mut self, text: String) {
        self.text = text;
        self.selection = ComposerSelectionState::None;
        self.last_pointer_click = None;
        self.composer_view_start = 0;
    }

    pub(in crate::tui) fn clear_text(&mut self) {
        self.text.clear();
        self.selection = ComposerSelectionState::None;
        self.last_pointer_click = None;
        self.composer_view_start = 0;
    }

    /// Clear a lone `/` that only opened the palette so Esc dismiss cannot trap blind retyping into `//cmd`.
    pub(in crate::tui) fn clear_if_bare_command_palette_opener(&mut self) {
        if self.text == "/" {
            self.clear_text();
            self.set_cursor(0);
        }
    }

    pub(in crate::tui) fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub(in crate::tui) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(in crate::tui) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }

    pub(in crate::tui) fn composer_view_start(&self) -> usize {
        self.composer_view_start
    }

    pub(in crate::tui) fn set_composer_view_start(&mut self, start: usize) {
        self.composer_view_start = start;
    }

    pub(in crate::tui) fn selection_focus(&self) -> Option<usize> {
        self.selection.value().map(ComposerSelection::focus)
    }

    pub(in crate::tui) fn selection_pointer_origin(&self) -> Option<usize> {
        self.selection
            .value()
            .map(ComposerSelection::pointer_origin)
    }

    pub(in crate::tui) fn selection_dragging(&self) -> bool {
        matches!(self.selection, ComposerSelectionState::Dragging(_))
    }

    /// Highlight/edit range when the selection spans at least one character.
    pub(in crate::tui) fn selection_range(&self) -> Option<std::ops::Range<usize>> {
        self.selection
            .value()
            .and_then(ComposerSelection::edit_range)
    }

    pub(in crate::tui) fn begin_selection(&mut self, position: usize) {
        self.selection = ComposerSelectionState::Dragging(ComposerSelection::characters(position));
    }

    /// Select an existing character range (for example double-click word select).
    ///
    /// Keeps the primary-button drag active so the user can extend the range.
    pub(in crate::tui) fn select_range(&mut self, start: usize, end: usize) {
        if start == end {
            self.clear_selection();
            return;
        }
        self.selection = ComposerSelectionState::Dragging(ComposerSelection::range(start, end));
    }

    pub(in crate::tui) fn update_selection(&mut self, position: usize) {
        if let ComposerSelectionState::Dragging(selection) = &mut self.selection {
            selection.update(position);
        }
    }

    /// Keep a non-empty selection after mouse release; drop a collapsed click.
    pub(in crate::tui) fn finalize_selection(&mut self) {
        self.selection = match self.selection {
            ComposerSelectionState::Dragging(selection) if selection.edit_range().is_some() => {
                ComposerSelectionState::Selected(selection)
            }
            ComposerSelectionState::Selected(selection) => {
                ComposerSelectionState::Selected(selection)
            }
            ComposerSelectionState::Dragging(_) | ComposerSelectionState::None => {
                ComposerSelectionState::None
            }
        };
    }

    pub(in crate::tui) fn clear_selection(&mut self) {
        self.selection = ComposerSelectionState::None;
    }

    /// Record a pointer press and consume a qualifying second press.
    pub(in crate::tui) fn register_pointer_click(
        &mut self,
        now: Instant,
        column: u16,
        row: u16,
        index: usize,
        maximum_gap: Duration,
    ) -> bool {
        let double_click = self.last_pointer_click.is_some_and(|click| {
            now.saturating_duration_since(click.at) <= maximum_gap
                && click.column == column
                && click.row == row
                && click.index == index
        });
        self.last_pointer_click = (!double_click).then_some(ComposerClick {
            at: now,
            column,
            row,
            index,
        });
        double_click
    }

    pub(in crate::tui) fn cancel_pointer_click_sequence(&mut self) {
        self.last_pointer_click = None;
    }

    /// Take a non-empty selection range and clear selection state.
    pub(in crate::tui) fn take_selection_range(&mut self) -> Option<std::ops::Range<usize>> {
        let range = self.selection_range();
        self.clear_selection();
        range
    }

    pub(in crate::tui) fn composer(&self) -> &ComposerMode {
        &self.composer
    }

    pub(in crate::tui) fn composer_mut(&mut self) -> &mut ComposerMode {
        &mut self.composer
    }

    pub(in crate::tui) fn set_composer(&mut self, composer: ComposerMode) {
        self.composer = composer;
        self.last_pointer_click = None;
        self.composer_view_start = 0;
    }

    pub(in crate::tui) fn take_composer(&mut self) -> ComposerMode {
        self.last_pointer_click = None;
        self.composer_view_start = 0;
        std::mem::replace(&mut self.composer, ComposerMode::Input)
    }

    pub(in crate::tui) fn paste_burst(&self) -> &PasteBurst {
        &self.paste_burst
    }

    /// Mutable paste-burst access for multi-step burst accumulation APIs.
    pub(in crate::tui) fn paste_burst_mut(&mut self) -> &mut PasteBurst {
        &mut self.paste_burst
    }

    pub(in crate::tui) fn clear_paste_burst(&mut self) {
        self.paste_burst.clear();
    }

    /// Clear short-lived edit bookkeeping tied to the composer (paste burst).
    pub(in crate::tui) fn clear_transient_edit_state(&mut self) {
        self.clear_paste_burst();
    }

    pub(in crate::tui) fn paste_segments(&self) -> &[PasteSegment] {
        &self.paste_segments
    }

    pub(in crate::tui) fn paste_segments_mut(&mut self) -> &mut Vec<PasteSegment> {
        &mut self.paste_segments
    }

    pub(in crate::tui) fn set_paste_segments(&mut self, segments: Vec<PasteSegment>) {
        self.paste_segments = segments;
    }

    pub(in crate::tui) fn clear_paste_segments(&mut self) {
        self.paste_segments.clear();
    }

    pub(in crate::tui) fn shell_mode(&self) -> Option<InlineShellMode> {
        self.shell_mode
    }

    pub(in crate::tui) fn shell_mode_mut(&mut self) -> &mut Option<InlineShellMode> {
        &mut self.shell_mode
    }

    pub(in crate::tui) fn set_shell_mode(&mut self, mode: Option<InlineShellMode>) {
        self.shell_mode = mode;
    }

    pub(in crate::tui) fn take_shell_mode(&mut self) -> Option<InlineShellMode> {
        self.shell_mode.take()
    }

    pub(in crate::tui) fn attachment_slots(&self) -> &[ComposerAttachmentSlot] {
        &self.attachments
    }

    pub(in crate::tui) fn attachment_epoch(&self) -> u64 {
        self.attachment_epoch
    }

    fn bump_attachments(&mut self) {
        self.attachment_epoch = self.attachment_epoch.wrapping_add(1);
    }

    /// Domain attachment list without render previews (tests and equality checks).
    pub(in crate::tui) fn attachments(&self) -> Vec<ComposerAttachment> {
        self.attachments
            .iter()
            .map(|slot| slot.attachment.clone())
            .collect()
    }

    #[cfg(test)]
    pub(in crate::tui) fn push_ready_attachment(
        &mut self,
        media: ChatMedia,
        image_preview: Option<FeedImage>,
    ) {
        self.attachments
            .push(ComposerAttachmentSlot::ready(media, image_preview));
        self.bump_attachments();
    }

    pub(in crate::tui) fn push_pending_attachment(
        &mut self,
        id: MediaAttachId,
        source: PendingAttachmentSource,
        name: String,
    ) {
        self.attachments
            .push(ComposerAttachmentSlot::pending(id, source, name));
        self.bump_attachments();
    }

    pub(in crate::tui) fn pop_attachment(&mut self) -> Option<ComposerAttachment> {
        let slot = self.attachments.pop()?;
        self.bump_attachments();
        Some(slot.attachment)
    }

    pub(in crate::tui) fn has_pending_attachments(&self) -> bool {
        self.attachments
            .iter()
            .any(|slot| slot.attachment.pending_id().is_some())
    }

    pub(in crate::tui) fn pending_attachment_count(&self) -> usize {
        self.attachments
            .iter()
            .filter(|slot| slot.attachment.pending_id().is_some())
            .count()
    }

    pub(in crate::tui) fn remove_pending_attachment(&mut self, id: MediaAttachId) -> Option<usize> {
        let index = self
            .attachments
            .iter()
            .position(|slot| slot.attachment.pending_id() == Some(id))?;
        self.attachments.remove(index);
        self.bump_attachments();
        Some(index)
    }

    pub(in crate::tui) fn replace_pending_attachment(
        &mut self,
        id: MediaAttachId,
        media: ChatMedia,
        image_preview: Option<FeedImage>,
    ) -> Option<usize> {
        let index = self
            .attachments
            .iter()
            .position(|slot| slot.attachment.pending_id() == Some(id))?;
        self.attachments[index] = ComposerAttachmentSlot::ready(media, image_preview);
        self.bump_attachments();
        Some(index)
    }

    pub(in crate::tui) fn take_ready_media(
        &mut self,
    ) -> Result<Vec<ChatMedia>, AttachmentsPending> {
        if self.has_pending_attachments() {
            return Err(AttachmentsPending);
        }
        self.bump_attachments();
        Ok(std::mem::take(&mut self.attachments)
            .into_iter()
            .map(|slot| match slot.attachment {
                ComposerAttachment::Ready(media) => media,
                ComposerAttachment::Pending { .. } => {
                    unreachable!("pending attachments checked before submission")
                }
            })
            .collect())
    }

    pub(in crate::tui) fn clear_attachments(&mut self) {
        self.attachments.clear();
        self.bump_attachments();
    }

    pub(in crate::tui) fn history(&self) -> &[String] {
        &self.history
    }

    pub(in crate::tui) fn push_history_if_new(&mut self, prompt: &str) -> bool {
        if self.history.last().is_some_and(|last| last == prompt) {
            return false;
        }
        self.history.push(prompt.to_string());
        true
    }

    pub(in crate::tui) fn seed_history_front(&mut self, mut entries: Vec<String>) {
        if entries.is_empty() {
            return;
        }
        if let (Some(last_seeded), Some(first_local)) = (entries.last(), self.history.first()) {
            if last_seeded == first_local {
                entries.pop();
            }
        }
        let inserted = entries.len();
        if inserted == 0 {
            return;
        }
        entries.append(&mut self.history);
        self.history = entries;
        if let Some(cursor) = &mut self.history_cursor {
            *cursor += inserted;
        }
    }

    pub(in crate::tui) fn clear_history(&mut self) {
        self.history.clear();
        self.reset_history_navigation();
    }

    pub(in crate::tui) fn truncate_history_to_newest(&mut self, max_entries: usize) {
        if max_entries == 0 || self.history.len() <= max_entries {
            return;
        }
        let drop = self.history.len() - max_entries;
        self.history.drain(..drop);
        self.reset_history_navigation();
    }

    pub(in crate::tui) fn history_cursor(&self) -> Option<usize> {
        self.history_cursor
    }

    pub(in crate::tui) fn set_history_cursor(&mut self, cursor: Option<usize>) {
        self.history_cursor = cursor;
    }

    pub(in crate::tui) fn set_history_draft(&mut self, draft: Option<InputDraft>) {
        self.history_draft = draft;
    }

    pub(in crate::tui) fn take_history_draft(&mut self) -> Option<InputDraft> {
        self.history_draft.take()
    }

    pub(in crate::tui) fn submission_mode(&self) -> InputSubmissionMode {
        self.submission_mode
    }

    pub(in crate::tui) fn set_submission_mode(&mut self, mode: InputSubmissionMode) {
        self.submission_mode = mode;
    }

    pub(in crate::tui) fn take_submission_mode(&mut self) -> InputSubmissionMode {
        std::mem::take(&mut self.submission_mode)
    }

    pub(in crate::tui) fn command_selection(&self) -> usize {
        self.command_selection
    }

    pub(in crate::tui) fn set_command_selection(&mut self, selection: usize) {
        self.command_selection = selection;
    }

    pub(in crate::tui) fn command_prefix(&self) -> Option<&str> {
        self.command_prefix.as_deref()
    }

    pub(in crate::tui) fn set_command_prefix(&mut self, prefix: Option<String>) {
        self.command_prefix = prefix;
    }

    pub(in crate::tui) fn command_palette_dismissed(&self) -> bool {
        self.command_palette_dismissed
    }

    pub(in crate::tui) fn set_command_palette_dismissed(&mut self, dismissed: bool) {
        self.command_palette_dismissed = dismissed;
    }

    pub(in crate::tui) fn file_selection(&self) -> usize {
        self.file_selection
    }

    pub(in crate::tui) fn set_file_selection(&mut self, selection: usize) {
        self.file_selection = selection;
    }

    pub(in crate::tui) fn file_query(&self) -> Option<&str> {
        self.file_query.as_deref()
    }

    pub(in crate::tui) fn set_file_query(&mut self, query: Option<String>) {
        self.file_query = query;
    }

    pub(in crate::tui) fn file_palette_dismissed(&self) -> bool {
        self.file_palette_dismissed
    }

    pub(in crate::tui) fn set_file_palette_dismissed(&mut self, dismissed: bool) {
        self.file_palette_dismissed = dismissed;
    }
}
