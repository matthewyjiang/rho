//! Cohesive App-owned UI state groups for history, composer input, and pending work.

use std::collections::VecDeque;
use std::time::Instant;

use rho_providers::model::ImageContent;

use super::{
    history_cache::HistoryLineCache,
    inline_shell::InlineShellMode,
    markdown_image,
    paste_burst::PasteBurst,
    pending_input::{AcceptedSteering, PendingInputAction, PendingInputPanel},
    scrollbar::HistoryScrollbarDrag,
    text_selection::{CopyNotice, TextSelection},
    ComposerMode, Entry, FileMatchCache, HistoryScroll, InputDraft, InputSubmissionMode,
    PasteSegment, QueuedPrompt, SessionHeaderCache, SkillMatchCache,
};

/// Transcript history, scroll, selection, and related render caches.
#[derive(Default)]
pub(super) struct HistoryUi {
    pub(in crate::tui) transcript: Vec<Entry>,
    pub(in crate::tui) history_lines: HistoryLineCache,
    pub(in crate::tui) last_status_notice: Option<String>,
    pub(in crate::tui) last_inserted_was_tool: bool,
    pub(in crate::tui) markdown_images: markdown_image::MarkdownImageCache,
    pub(in crate::tui) markdown_images_dirty_from: Option<usize>,
    pub(in crate::tui) history_scroll: HistoryScroll,
    pub(in crate::tui) history_scrollbar_drag: Option<HistoryScrollbarDrag>,
    pub(in crate::tui) history_scrollbar_visible_until: Option<Instant>,
    pub(in crate::tui) history_scrollbar_hovered: bool,
    pub(in crate::tui) hovered_code_block_copy: Option<usize>,
    pub(in crate::tui) text_selection: Option<TextSelection>,
    pub(in crate::tui) copy_notice: Option<CopyNotice>,
    pub(in crate::tui) session_header_cache: Option<SessionHeaderCache>,
}

impl HistoryUi {
    /// Drop cached history lines and mark markdown images dirty from `index`.
    pub(super) fn invalidate_from(&mut self, index: usize) {
        self.history_lines.invalidate_from(index);
        self.markdown_images_dirty_from = Some(
            self.markdown_images_dirty_from
                .map_or(index, |dirty_from| dirty_from.min(index)),
        );
    }
}

/// Composer text, paste handling, command/file palettes, and input history.
#[derive(Default)]
pub(super) struct InputUi {
    pub(in crate::tui) input: String,
    pub(in crate::tui) input_cursor: usize,
    pub(in crate::tui) shell_mode: Option<InlineShellMode>,
    pub(in crate::tui) pending_images: Vec<ImageContent>,
    pub(in crate::tui) input_history: Vec<String>,
    pub(in crate::tui) input_history_cursor: Option<usize>,
    pub(in crate::tui) input_history_draft: Option<InputDraft>,
    pub(in crate::tui) paste_burst: PasteBurst,
    pub(in crate::tui) paste_segments: Vec<PasteSegment>,
    pub(in crate::tui) input_submission_mode: InputSubmissionMode,
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

/// Queued prompts, steering, and the pending-input panel.
#[derive(Default)]
pub(super) struct PendingWorkUi {
    pub(in crate::tui) steering_prompts: VecDeque<QueuedPrompt>,
    pub(in crate::tui) accepted_steering: VecDeque<AcceptedSteering>,
    pub(in crate::tui) retracting_steering: Option<rho_sdk::SteeringId>,
    pub(in crate::tui) pending_input_panel: PendingInputPanel,
    pub(in crate::tui) pending_input_action: Option<PendingInputAction>,
    pub(in crate::tui) queued_prompts: VecDeque<QueuedPrompt>,
}
