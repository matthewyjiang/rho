//! Cohesive App-owned UI state groups for history, composer input, pending work,
//! and the live turn.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rho_providers::model::ImageContent;

use super::{
    activity::{ActivityPhase, LoadingSpinner},
    history_cache::HistoryLineCache,
    inline_shell::InlineShellMode,
    markdown_image,
    paste_burst::{expand_paste_segments, PasteBurst},
    pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel},
    provider_attempt::ProviderAttempt,
    reasoning_phase::ReasoningPhase,
    scrollbar::HistoryScrollbarDrag,
    text_selection::{CopyNotice, TextSelection},
    tool_call_batch::ToolCallBatch,
    ComposerMode, Entry, FileMatchCache, HistoryScroll, InputDraft, InputSubmissionMode,
    PasteSegment, QueuedPrompt, SessionHeaderCache, SkillMatchCache,
};

/// TUI session phase distinct from provider run controller state.
///
/// `ProviderTurn` should stay aligned with `InteractiveRuntime::is_run_active`
/// except for brief setup before `start` succeeds. `Compacting` is UI-only busy
/// work with no active provider run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SessionUiPhase {
    #[default]
    Idle,
    ProviderTurn,
    Compacting,
}

impl SessionUiPhase {
    pub(super) const fn is_busy(self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(super) const fn is_provider_turn(self) -> bool {
        matches!(self, Self::ProviderTurn)
    }

    pub(super) const fn allows_idle_subagent_delivery(self) -> bool {
        matches!(self, Self::Idle)
    }

    pub(super) const fn uses_during_run_model_picker(self) -> bool {
        matches!(self, Self::ProviderTurn)
    }

    pub(super) const fn busy_status_label(self) -> &'static str {
        if self.is_busy() {
            "running"
        } else {
            "ready"
        }
    }
}

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

    pub(super) fn set_text_and_cursor(&mut self, text: String, cursor: usize) {
        self.text = text;
        self.cursor = cursor;
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn text_mut(&mut self) -> &mut String {
        &mut self.text
    }

    pub(super) fn set_text(&mut self, text: String) {
        self.text = text;
    }

    pub(super) fn clear_text(&mut self) {
        self.text.clear();
    }

    pub(super) fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(super) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }

    pub(super) fn composer(&self) -> &ComposerMode {
        &self.composer
    }

    pub(super) fn composer_mut(&mut self) -> &mut ComposerMode {
        &mut self.composer
    }

    pub(super) fn set_composer(&mut self, composer: ComposerMode) {
        self.composer = composer;
    }

    pub(super) fn take_composer(&mut self) -> ComposerMode {
        std::mem::replace(&mut self.composer, ComposerMode::Input)
    }

    pub(super) fn paste_burst(&self) -> &PasteBurst {
        &self.paste_burst
    }

    pub(super) fn paste_burst_mut(&mut self) -> &mut PasteBurst {
        &mut self.paste_burst
    }

    pub(super) fn paste_segments(&self) -> &[PasteSegment] {
        &self.paste_segments
    }

    pub(super) fn paste_segments_mut(&mut self) -> &mut Vec<PasteSegment> {
        &mut self.paste_segments
    }

    pub(super) fn set_paste_segments(&mut self, segments: Vec<PasteSegment>) {
        self.paste_segments = segments;
    }

    pub(super) fn clear_paste_segments(&mut self) {
        self.paste_segments.clear();
    }

    pub(super) fn shell_mode(&self) -> Option<InlineShellMode> {
        self.shell_mode
    }

    pub(super) fn shell_mode_mut(&mut self) -> &mut Option<InlineShellMode> {
        &mut self.shell_mode
    }

    pub(super) fn set_shell_mode(&mut self, mode: Option<InlineShellMode>) {
        self.shell_mode = mode;
    }

    pub(super) fn take_shell_mode(&mut self) -> Option<InlineShellMode> {
        self.shell_mode.take()
    }

    pub(super) fn pending_images(&self) -> &[ImageContent] {
        &self.pending_images
    }

    pub(super) fn pending_images_mut(&mut self) -> &mut Vec<ImageContent> {
        &mut self.pending_images
    }

    pub(super) fn clear_pending_images(&mut self) {
        self.pending_images.clear();
    }

    pub(super) fn history(&self) -> &[String] {
        &self.history
    }

    pub(super) fn push_history_if_new(&mut self, prompt: &str) {
        if self.history.last().is_some_and(|last| last == prompt) {
            return;
        }
        self.history.push(prompt.to_string());
    }

    pub(super) fn history_cursor(&self) -> Option<usize> {
        self.history_cursor
    }

    pub(super) fn set_history_cursor(&mut self, cursor: Option<usize>) {
        self.history_cursor = cursor;
    }

    pub(super) fn set_history_draft(&mut self, draft: Option<InputDraft>) {
        self.history_draft = draft;
    }

    pub(super) fn take_history_draft(&mut self) -> Option<InputDraft> {
        self.history_draft.take()
    }

    pub(super) fn submission_mode(&self) -> InputSubmissionMode {
        self.submission_mode
    }

    pub(super) fn set_submission_mode(&mut self, mode: InputSubmissionMode) {
        self.submission_mode = mode;
    }

    pub(super) fn take_submission_mode(&mut self) -> InputSubmissionMode {
        std::mem::take(&mut self.submission_mode)
    }

    pub(super) fn command_selection(&self) -> usize {
        self.command_selection
    }

    pub(super) fn set_command_selection(&mut self, selection: usize) {
        self.command_selection = selection;
    }

    pub(super) fn command_prefix(&self) -> Option<&str> {
        self.command_prefix.as_deref()
    }

    pub(super) fn set_command_prefix(&mut self, prefix: Option<String>) {
        self.command_prefix = prefix;
    }

    pub(super) fn command_palette_dismissed(&self) -> bool {
        self.command_palette_dismissed
    }

    pub(super) fn set_command_palette_dismissed(&mut self, dismissed: bool) {
        self.command_palette_dismissed = dismissed;
    }

    pub(super) fn file_selection(&self) -> usize {
        self.file_selection
    }

    pub(super) fn set_file_selection(&mut self, selection: usize) {
        self.file_selection = selection;
    }

    pub(super) fn file_query(&self) -> Option<&str> {
        self.file_query.as_deref()
    }

    pub(super) fn set_file_query(&mut self, query: Option<String>) {
        self.file_query = query;
    }

    pub(super) fn file_palette_dismissed(&self) -> bool {
        self.file_palette_dismissed
    }

    pub(super) fn set_file_palette_dismissed(&mut self, dismissed: bool) {
        self.file_palette_dismissed = dismissed;
    }

    pub(super) fn file_match_cache(&self) -> Option<&FileMatchCache> {
        self.file_match_cache.as_ref()
    }

    pub(super) fn file_match_cache_mut(&mut self) -> &mut Option<FileMatchCache> {
        &mut self.file_match_cache
    }

    pub(super) fn set_file_match_cache(&mut self, cache: Option<FileMatchCache>) {
        self.file_match_cache = cache;
    }

    pub(super) fn skill_match_cache(&self) -> Option<&SkillMatchCache> {
        self.skill_match_cache.as_ref()
    }

    pub(super) fn set_skill_match_cache(&mut self, cache: Option<SkillMatchCache>) {
        self.skill_match_cache = cache;
    }
}

/// Queued prompts, steering, and the pending-input panel.
#[derive(Default)]
pub(super) struct PendingWorkUi {
    steering_prompts: VecDeque<QueuedPrompt>,
    accepted_steering: VecDeque<AcceptedSteering>,
    retracting_steering: Option<rho_sdk::SteeringId>,
    input_panel: PendingInputPanel,
    input_action: Option<PendingInputAction>,
    queued_prompts: VecDeque<QueuedPrompt>,
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

    pub(super) fn steering_prompts(&self) -> &VecDeque<QueuedPrompt> {
        &self.steering_prompts
    }

    pub(super) fn steering_prompts_mut(&mut self) -> &mut VecDeque<QueuedPrompt> {
        &mut self.steering_prompts
    }

    pub(super) fn accepted_steering(&self) -> &VecDeque<AcceptedSteering> {
        &self.accepted_steering
    }

    pub(super) fn accepted_steering_mut(&mut self) -> &mut VecDeque<AcceptedSteering> {
        &mut self.accepted_steering
    }

    pub(super) fn queued_prompts(&self) -> &VecDeque<QueuedPrompt> {
        &self.queued_prompts
    }

    pub(super) fn queued_prompts_mut(&mut self) -> &mut VecDeque<QueuedPrompt> {
        &mut self.queued_prompts
    }

    pub(super) fn push_follow_up(&mut self, prompt: QueuedPrompt) {
        self.queued_prompts.push_back(prompt);
    }

    pub(super) fn clear_steering(&mut self) {
        self.steering_prompts.clear();
        self.accepted_steering.clear();
        self.retracting_steering = None;
    }

    pub(super) fn retracting_steering(&self) -> Option<&rho_sdk::SteeringId> {
        self.retracting_steering.as_ref()
    }

    pub(super) fn set_retracting_steering(&mut self, id: Option<rho_sdk::SteeringId>) {
        self.retracting_steering = id;
    }

    pub(super) fn input_panel(&self) -> &PendingInputPanel {
        &self.input_panel
    }

    pub(super) fn input_panel_mut(&mut self) -> &mut PendingInputPanel {
        &mut self.input_panel
    }

    pub(super) fn input_action(&self) -> Option<&PendingInputAction> {
        self.input_action.as_ref()
    }

    pub(super) fn set_input_action(&mut self, action: Option<PendingInputAction>) {
        self.input_action = action;
    }

    pub(super) fn take_input_action(&mut self) -> Option<PendingInputAction> {
        self.input_action.take()
    }

    pub(super) fn clear_input_action(&mut self) {
        self.input_action = None;
    }

    pub(super) fn drain_accepted_steering_prompts(&mut self) -> impl Iterator<Item = String> + '_ {
        self.accepted_steering
            .drain(..)
            .map(|entry| entry.prompt.prompt)
    }

    pub(super) fn drain_steering_prompt_texts(&mut self) -> impl Iterator<Item = String> + '_ {
        self.steering_prompts.drain(..).map(|prompt| prompt.prompt)
    }

    pub(super) fn drain_queued_prompt_texts(&mut self) -> impl Iterator<Item = String> + '_ {
        self.queued_prompts.drain(..).map(|prompt| prompt.prompt)
    }
}

/// Live-turn UI: provider attempt, activity, spinner, and in-flight tools.
#[derive(Default)]
pub(super) struct TurnUi {
    current_turn_start: Option<usize>,
    provider_attempt: ProviderAttempt,
    reasoning_phase: ReasoningPhase,
    session_ui: SessionUiPhase,
    activity_phase: ActivityPhase,
    loading_spinner: LoadingSpinner,
    tool_calls: ToolCallBatch,
}

impl TurnUi {
    pub(super) fn current_turn_start(&self) -> Option<usize> {
        self.current_turn_start
    }

    pub(super) fn set_current_turn_start(&mut self, start: Option<usize>) {
        self.current_turn_start = start;
    }

    pub(super) fn provider_attempt_mut(&mut self) -> &mut ProviderAttempt {
        &mut self.provider_attempt
    }

    pub(super) fn reasoning_phase(&self) -> &ReasoningPhase {
        &self.reasoning_phase
    }

    pub(super) fn reasoning_phase_mut(&mut self) -> &mut ReasoningPhase {
        &mut self.reasoning_phase
    }

    pub(super) fn session_ui(&self) -> SessionUiPhase {
        self.session_ui
    }

    pub(super) fn set_session_ui(&mut self, phase: SessionUiPhase) {
        self.session_ui = phase;
    }

    pub(super) fn activity_phase(&self) -> ActivityPhase {
        self.activity_phase
    }

    pub(super) fn set_activity_phase(&mut self, phase: ActivityPhase) {
        self.activity_phase = phase;
    }

    pub(super) fn loading_spinner(&self) -> &LoadingSpinner {
        &self.loading_spinner
    }

    pub(super) fn loading_spinner_mut(&mut self) -> &mut LoadingSpinner {
        &mut self.loading_spinner
    }

    pub(super) fn tool_calls(&self) -> &ToolCallBatch {
        &self.tool_calls
    }

    pub(super) fn tool_calls_mut(&mut self) -> &mut ToolCallBatch {
        &mut self.tool_calls
    }
}
