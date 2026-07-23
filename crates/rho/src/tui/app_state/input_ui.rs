//! Composer text, paste handling, command/file palettes, and input history.

use rho_providers::model::ImageContent;

use crate::tui::{
    inline_shell::InlineShellMode,
    paste_burst::{expand_paste_segments, PasteBurst},
    ComposerMode, FileMatchCache, InputDraft, InputSubmissionMode, PasteSegment, SkillMatchCache,
};

/// Composer text, paste handling, command/file palettes, and input history.
#[derive(Default)]
pub(in crate::tui) struct InputUi {
    text: String,
    cursor: usize,
    shell_mode: Option<InlineShellMode>,
    pending_images: Vec<ImageContent>,
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
        self.pending_images.clear();
    }

    pub(in crate::tui) fn expanded_text(&self) -> String {
        expand_paste_segments(&self.text, &self.paste_segments)
    }

    pub(in crate::tui) fn reset_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
    }

    pub(in crate::tui) fn set_text_and_cursor(&mut self, text: String, cursor: usize) {
        self.text = text;
        self.cursor = cursor;
    }

    pub(in crate::tui) fn apply_input_draft(&mut self, draft: InputDraft) {
        self.shell_mode = draft.shell_mode;
        self.text = draft.input;
        self.paste_segments = draft.paste_segments;
        self.submission_mode = draft.submission_mode;
        self.cursor = self.text.chars().count();
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
    }

    pub(in crate::tui) fn clear_text(&mut self) {
        self.text.clear();
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

    pub(in crate::tui) fn pending_images(&self) -> &[ImageContent] {
        &self.pending_images
    }

    pub(in crate::tui) fn pending_images_mut(&mut self) -> &mut Vec<ImageContent> {
        &mut self.pending_images
    }

    pub(in crate::tui) fn clear_pending_images(&mut self) {
        self.pending_images.clear();
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
