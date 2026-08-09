//! Composer text, paste handling, command/file palettes, and input history.

use crate::tui::{
    inline_shell::InlineShellMode,
    paste_burst::{expand_paste_segments, PasteBurst},
    ChatMedia, ComposerAttachment, ComposerMode, FileMatchCache, InputDraft, InputSubmissionMode,
    MediaAttachId, PasteSegment, SkillMatchCache,
};

#[derive(Debug)]
pub(in crate::tui) struct AttachmentsPending;

/// Editable character-range selection inside the free-text composer.
///
/// `anchor` is where the pointer went down; `focus` tracks the live end while
/// dragging and after release. A collapsed range (`anchor == focus`) is not a
/// selection for editing purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ComposerSelection {
    anchor: usize,
    focus: usize,
}

impl ComposerSelection {
    pub(in crate::tui) fn new(position: usize) -> Self {
        Self {
            anchor: position,
            focus: position,
        }
    }

    pub(in crate::tui) fn from_range(start: usize, end: usize) -> Self {
        Self {
            anchor: start,
            focus: end,
        }
    }

    pub(in crate::tui) fn update(&mut self, position: usize) {
        self.focus = position;
    }

    pub(in crate::tui) fn focus(self) -> usize {
        self.focus
    }

    pub(in crate::tui) fn has_range(self) -> bool {
        self.anchor != self.focus
    }

    /// Ordered half-open char range when the selection spans text.
    pub(in crate::tui) fn range(self) -> Option<std::ops::Range<usize>> {
        self.has_range().then_some(if self.anchor <= self.focus {
            self.anchor..self.focus
        } else {
            self.focus..self.anchor
        })
    }
}

/// Composer text, paste handling, command/file palettes, and input history.
#[derive(Default)]
pub(in crate::tui) struct InputUi {
    text: String,
    cursor: usize,
    /// Mouse/keyboard text selection within [`Self::text`] (char indices).
    selection: Option<ComposerSelection>,
    /// True only while the primary button is held after a composer press.
    selection_dragging: bool,
    shell_mode: Option<InlineShellMode>,
    attachments: Vec<ComposerAttachment>,
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
    file_match_cache: Option<FileMatchCache>,
    skill_match_cache: Option<SkillMatchCache>,
    composer: ComposerMode,
}

impl InputUi {
    /// Clear composer text state after a successful submit.
    pub(in crate::tui) fn clear_submitted(&mut self) {
        self.text.clear();
        self.paste_segments.clear();
        self.shell_mode = None;
        self.cursor = 0;
        self.selection = None;
        self.selection_dragging = false;
        self.attachments.clear();
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
        self.selection = None;
        self.selection_dragging = false;
    }

    pub(in crate::tui) fn apply_input_draft(&mut self, draft: InputDraft) {
        self.shell_mode = draft.shell_mode;
        self.text = draft.input;
        self.paste_segments = draft.paste_segments;
        self.submission_mode = draft.submission_mode;
        self.cursor = self.text.chars().count();
        self.selection = None;
        self.selection_dragging = false;
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
        self.selection = None;
        self.selection_dragging = false;
    }

    pub(in crate::tui) fn clear_text(&mut self) {
        self.text.clear();
        self.selection = None;
        self.selection_dragging = false;
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

    pub(in crate::tui) fn selection(&self) -> Option<ComposerSelection> {
        self.selection
    }

    pub(in crate::tui) fn selection_dragging(&self) -> bool {
        self.selection_dragging
    }

    /// Highlight/edit range when the selection spans at least one character.
    pub(in crate::tui) fn selection_range(&self) -> Option<std::ops::Range<usize>> {
        self.selection.and_then(ComposerSelection::range)
    }

    pub(in crate::tui) fn begin_selection(&mut self, position: usize) {
        self.selection = Some(ComposerSelection::new(position));
        self.selection_dragging = true;
    }

    /// Select an existing character range (for example double-click word select).
    ///
    /// Keeps the primary-button drag active so the user can extend the range.
    pub(in crate::tui) fn select_range(&mut self, start: usize, end: usize) {
        if start == end {
            self.clear_selection();
            return;
        }
        self.selection = Some(ComposerSelection::from_range(start, end));
        self.selection_dragging = true;
    }

    pub(in crate::tui) fn update_selection(&mut self, position: usize) {
        if !self.selection_dragging {
            return;
        }
        if let Some(selection) = self.selection.as_mut() {
            selection.update(position);
        }
    }

    /// Keep a non-empty selection after mouse release; drop a collapsed click.
    pub(in crate::tui) fn finalize_selection(&mut self) {
        self.selection_dragging = false;
        if self
            .selection
            .is_some_and(|selection| !selection.has_range())
        {
            self.selection = None;
        }
    }

    pub(in crate::tui) fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_dragging = false;
    }

    /// Take a non-empty selection range and clear selection state.
    pub(in crate::tui) fn take_selection_range(&mut self) -> Option<std::ops::Range<usize>> {
        let range = self.selection_range()?;
        self.clear_selection();
        Some(range)
    }

    pub(in crate::tui) fn composer(&self) -> &ComposerMode {
        &self.composer
    }

    pub(in crate::tui) fn composer_mut(&mut self) -> &mut ComposerMode {
        &mut self.composer
    }

    pub(in crate::tui) fn set_composer(&mut self, composer: ComposerMode) {
        self.composer = composer;
    }

    pub(in crate::tui) fn take_composer(&mut self) -> ComposerMode {
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

    pub(in crate::tui) fn attachments(&self) -> &[ComposerAttachment] {
        &self.attachments
    }

    pub(in crate::tui) fn push_ready_attachment(&mut self, media: ChatMedia) {
        self.attachments.push(ComposerAttachment::Ready(media));
    }

    pub(in crate::tui) fn push_pending_attachment(&mut self, id: MediaAttachId, name: String) {
        self.attachments
            .push(ComposerAttachment::Pending { id, name });
    }

    pub(in crate::tui) fn pop_attachment(&mut self) -> Option<ComposerAttachment> {
        self.attachments.pop()
    }

    pub(in crate::tui) fn has_pending_attachments(&self) -> bool {
        self.attachments
            .iter()
            .any(|attachment| attachment.pending_id().is_some())
    }

    pub(in crate::tui) fn pending_attachment_count(&self) -> usize {
        self.attachments
            .iter()
            .filter(|attachment| attachment.pending_id().is_some())
            .count()
    }

    pub(in crate::tui) fn remove_pending_attachment(&mut self, id: MediaAttachId) -> Option<usize> {
        let index = self
            .attachments
            .iter()
            .position(|attachment| attachment.pending_id() == Some(id))?;
        self.attachments.remove(index);
        Some(index)
    }

    pub(in crate::tui) fn replace_pending_attachment(
        &mut self,
        id: MediaAttachId,
        media: ChatMedia,
    ) -> Option<usize> {
        let index = self
            .attachments
            .iter()
            .position(|attachment| attachment.pending_id() == Some(id))?;
        self.attachments[index] = ComposerAttachment::Ready(media);
        Some(index)
    }

    pub(in crate::tui) fn take_ready_media(
        &mut self,
    ) -> Result<Vec<ChatMedia>, AttachmentsPending> {
        if self.has_pending_attachments() {
            return Err(AttachmentsPending);
        }
        Ok(std::mem::take(&mut self.attachments)
            .into_iter()
            .map(|attachment| match attachment {
                ComposerAttachment::Ready(media) => media,
                ComposerAttachment::Pending { .. } => {
                    unreachable!("pending attachments checked before submission")
                }
            })
            .collect())
    }

    pub(in crate::tui) fn clear_attachments(&mut self) {
        self.attachments.clear();
    }

    pub(in crate::tui) fn history(&self) -> &[String] {
        &self.history
    }

    pub(in crate::tui) fn push_history_if_new(&mut self, prompt: &str) {
        if self.history.last().is_some_and(|last| last == prompt) {
            return;
        }
        self.history.push(prompt.to_string());
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

    pub(in crate::tui) fn file_match_cache(&self) -> Option<&FileMatchCache> {
        self.file_match_cache.as_ref()
    }

    pub(in crate::tui) fn file_match_cache_mut(&mut self) -> &mut Option<FileMatchCache> {
        &mut self.file_match_cache
    }

    pub(in crate::tui) fn set_file_match_cache(&mut self, cache: Option<FileMatchCache>) {
        self.file_match_cache = cache;
    }

    pub(in crate::tui) fn skill_match_cache(&self) -> Option<&SkillMatchCache> {
        self.skill_match_cache.as_ref()
    }

    pub(in crate::tui) fn set_skill_match_cache(&mut self, cache: Option<SkillMatchCache>) {
        self.skill_match_cache = cache;
    }
}
