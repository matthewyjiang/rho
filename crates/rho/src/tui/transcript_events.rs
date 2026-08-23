//! Live provider stream and transcript event handling for the interactive TUI.
//!
//! This module owns App methods that switch and drain assistant/reasoning
//! streams, schedule stream previews, record usage and cost from view-model
//! events, drive tool-call lifecycle display state, and merge finished text
//! into the transcript. Expand/collapse of truncated tool output lives in
//! `tool_output_ui`. Stream finalization that must happen before recording a
//! lifecycle event is classified exhaustively via
//! [`should_finish_streams_before_recording`].

use std::time::Instant;

use ratatui::{backend::Backend, DefaultTerminal, Terminal};

use super::{
    activity::ActivityPhase,
    event_adapter::ViewModelEvent,
    markdown::update_code_block_state,
    render::padded_content_width,
    stream::StreamFragment,
    usage_cost::{
        add_optional, merge_usage, usage_difference, usage_with_estimated_cost, CostSource,
    },
    App, Entry, FinalAnswerDelta, LiveStreamPreview, ReasoningEntry, StreamKind, ToolEntry,
};
use rho_providers::model::ContentBlock;

pub(super) fn final_answer_delta<'a>(emitted_text: &str, answer: &'a str) -> FinalAnswerDelta<'a> {
    match answer.strip_prefix(emitted_text) {
        Some("") => FinalAnswerDelta::None,
        Some(suffix) => FinalAnswerDelta::Append(suffix),
        None => FinalAnswerDelta::Mismatch,
    }
}

fn should_finish_streams_before_recording(event: &ViewModelEvent) -> bool {
    match event {
        ViewModelEvent::StepStarted(_)
        | ViewModelEvent::ToolCallUpdated { .. }
        | ViewModelEvent::ToolCallProposed { .. }
        | ViewModelEvent::ToolStarted { .. }
        | ViewModelEvent::ToolFinished { .. } => true,
        ViewModelEvent::RunStarted
        | ViewModelEvent::SteeringApplied(_)
        | ViewModelEvent::ProviderStreamReset(_)
        | ViewModelEvent::ProviderRetry
        | ViewModelEvent::OutputDelta(_)
        | ViewModelEvent::ReasoningDelta(_)
        | ViewModelEvent::LiveOutputText(_)
        | ViewModelEvent::ContextUsage(_)
        | ViewModelEvent::Usage(_)
        | ViewModelEvent::ModelCallCompleted { .. }
        | ViewModelEvent::ToolUpdated { .. } => false,
    }
}

impl App {
    pub(super) fn reset_streams(&mut self) {
        self.streams.reset();
        // Discard an unfinished reasoning phase. Callers that should keep a
        // summary must finalize before reset (for example `finish_streams`).
        self.turn.reasoning_phase_mut().reset();
    }

    pub(super) fn handle_agent_event<B: Backend>(
        &mut self,
        event: ViewModelEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        if let Some(phase) = event.activity_phase() {
            self.turn.set_activity_phase(phase);
        }
        match event {
            ViewModelEvent::OutputDelta(text) => {
                self.usage.live_stream.add_output_text(&text);
                let switched = self.switch_stream_kind(StreamKind::Assistant);
                let drained = self.receive_stream_delta(terminal, StreamKind::Assistant, &text)?;
                Ok(switched || drained)
            }
            ViewModelEvent::ReasoningDelta(text) => {
                self.usage.live_stream.add_output_text(&text);
                self.turn.reasoning_phase_mut().on_reasoning_delta();
                if !self.info.runtime.displays_reasoning_output() {
                    return Ok(true);
                }
                let switched = self.switch_stream_kind(StreamKind::Reasoning);
                let drained = self.receive_stream_delta(terminal, StreamKind::Reasoning, &text)?;
                Ok(switched || drained)
            }
            other => {
                if should_finish_streams_before_recording(&other) {
                    self.finish_streams();
                }
                if let Some(entry) = self.record_agent_event(other) {
                    self.insert_entry(&entry);
                }
                self.drain_streams(terminal)?;
                Ok(true)
            }
        }
    }

    pub(super) fn switch_stream_kind(&mut self, kind: StreamKind) -> bool {
        let inserted = if self
            .streams
            .current_stream_kind
            .is_some_and(|current| current != kind)
        {
            self.finish_current_stream()
        } else {
            false
        };
        // Closing into assistant ends the reasoning phase so the thought
        // footer lands after any finished reasoning text.
        let thought = if kind == StreamKind::Assistant
            && self.streams.current_stream_kind != Some(StreamKind::Assistant)
        {
            self.close_reasoning_phase()
        } else {
            false
        };
        self.streams.current_stream_kind = Some(kind);
        self.streams.schedule_tick(kind, Instant::now());
        inserted || thought
    }

    pub(super) fn drain_streams<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        let reasoning_drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
        let assistant_drained = self.drain_stream(terminal, StreamKind::Assistant)?;
        Ok(reasoning_drained || assistant_drained)
    }

    /// Appends one provider delta through the hold/pacer and commits renderable lines.
    fn receive_stream_delta<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        kind: StreamKind,
        text: &str,
    ) -> Result<bool, B::Error> {
        self.streams.push_delta(kind, text, Instant::now());
        self.drain_stream(terminal, kind)
    }

    pub(super) fn drain_stream<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        kind: StreamKind,
    ) -> Result<bool, B::Error> {
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let in_code_block = self.streams.code_fence(kind).is_open();
        let fragment = self
            .streams
            .stream_mut(kind)
            .drain_renderable_markdown(inner_width, in_code_block);
        if let Some(fragment) = fragment {
            self.streams.live_stream_preview = None;
            self.insert_stream_fragment(fragment, kind);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn finish_current_stream(&mut self) -> bool {
        self.streams
            .current_stream_kind
            .is_some_and(|kind| self.finish_stream(kind))
    }

    /// Releases held text and refreshes the partial-line preview on the shared
    /// stream UI cadence.
    pub(super) fn drain_stream_tick(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<bool> {
        let now = Instant::now();
        if self
            .streams
            .stream_tick_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return Ok(false);
        }
        let released = self.streams.on_tick(now);
        let Some(kind) = self.streams.current_stream_kind else {
            return Ok(false);
        };
        let drained = if released {
            self.drain_stream(terminal, kind)?
        } else {
            false
        };
        let preview_changed = self.refresh_stream_preview(terminal, kind)?;
        Ok(drained || preview_changed)
    }

    fn refresh_stream_preview(
        &mut self,
        terminal: &mut DefaultTerminal,
        kind: StreamKind,
    ) -> std::io::Result<bool> {
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let in_code_block = self.streams.code_fence(kind).is_open();
        let preview = self
            .streams
            .stream(kind)
            .drain_preview_markdown(inner_width, in_code_block);
        if let Some(preview) = preview {
            self.streams.live_stream_preview = Some(LiveStreamPreview {
                kind,
                text: preview.render_text().to_string(),
                include_leading_blank: preview.include_leading_blank(),
            });
            Ok(true)
        } else if self.streams.live_stream_preview.is_some() {
            self.streams.live_stream_preview = None;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn record_agent_event(&mut self, event: ViewModelEvent) -> Option<Entry> {
        if event.rewrites_prompt_prefix() {
            self.usage.cache_stats.prompt_prefix_reset();
        }
        match event {
            ViewModelEvent::RunStarted => {
                self.usage.usage_cost_tracker.run_started();
                self.usage.usage_before_current_run = self.usage.cumulative_usage.clone();
                self.usage.run_usage.clear();
                self.usage.live_stream.clear();
                None
            }
            ViewModelEvent::StepStarted(step) => {
                self.usage.usage_cost_tracker.step_started();
                self.usage.run_usage.step_started();
                self.usage.live_stream.clear();
                self.reset_streams();
                self.turn.provider_attempt_mut().begin(self.history.len());
                self.turn.reasoning_phase_mut().begin_step();
                self.begin_provider_turn_ui();
                self.turn.clear_tool_calls();
                self.turn.start_loading_if_needed();
                self.set_status(format!("running step {step}"));
                None
            }
            ViewModelEvent::SteeringApplied(ids) => {
                self.record_applied_steering(&ids);
                None
            }
            ViewModelEvent::ToolStarted { call_id, card } => {
                self.turn.tool_started(call_id, card);
                None
            }
            ViewModelEvent::ToolUpdated { call_id, card } => {
                self.turn.tool_updated(call_id, card);
                None
            }
            ViewModelEvent::ToolCallUpdated {
                index,
                call_id,
                card,
            } => {
                self.turn.tool_call_preview(index, call_id, card);
                None
            }
            ViewModelEvent::ToolCallProposed { call_id, card } => {
                self.turn.tool_call_proposed(call_id, card);
                None
            }
            ViewModelEvent::ProviderStreamReset(retry) => {
                self.reset_provider_attempt_stream(retry);
                self.reset_attempt_accounting();
                self.usage.live_stream.clear();
                None
            }
            ViewModelEvent::ProviderRetry => {
                self.reset_attempt_accounting();
                self.usage.live_stream.clear();
                None
            }
            ViewModelEvent::OutputDelta(_) | ViewModelEvent::ReasoningDelta(_) => None,
            ViewModelEvent::LiveOutputText(text) => {
                self.usage.live_stream.add_output_text(&text);
                None
            }
            ViewModelEvent::ContextUsage(usage) => {
                self.info.services.diagnostics.record_context(usage.clone());
                self.usage.current_context = Some(usage);
                None
            }
            ViewModelEvent::ModelCallCompleted {
                profile,
                metrics,
                generation_output_tokens,
            } => {
                self.usage.cache_stats.record_request(
                    &profile,
                    metrics,
                    self.model_metadata.as_ref(),
                    Instant::now(),
                );
                self.usage
                    .model_performance
                    .record(profile, metrics, generation_output_tokens);
                None
            }
            ViewModelEvent::Usage(usage) => {
                self.usage.live_stream.provider_usage_received();
                let current_cost_source = self.usage.usage_cost_tracker.record_usage(&usage);
                let model_metadata = self.model_metadata.as_ref();
                let mut current_run_usage =
                    self.usage.run_usage.apply_snapshot(usage, |snapshot| {
                        usage_with_estimated_cost(snapshot, model_metadata)
                    });
                let step_baseline = self
                    .usage
                    .run_usage
                    .before_step()
                    .cloned()
                    .map(|usage| usage_with_estimated_cost(usage, model_metadata));
                let mut latest_usage = usage_difference(&current_run_usage, step_baseline.as_ref());
                latest_usage = usage_with_estimated_cost(latest_usage, model_metadata);
                if current_cost_source == CostSource::Estimated {
                    current_run_usage.cost_usd_micros = add_optional(
                        step_baseline
                            .as_ref()
                            .and_then(|usage| usage.cost_usd_micros),
                        latest_usage.cost_usd_micros,
                    );
                    if let Some(current) = self.usage.run_usage.current_mut() {
                        current.cost_usd_micros = current_run_usage.cost_usd_micros;
                    }
                }
                self.usage.cache_stats.usage_updated(&latest_usage);
                self.usage.latest_usage = Some(latest_usage);
                self.usage
                    .cumulative_usage
                    .clone_from(&self.usage.usage_before_current_run);
                merge_usage(&mut self.usage.cumulative_usage, current_run_usage);
                None
            }
            ViewModelEvent::ToolFinished {
                call_id,
                mut card,
                image_asset,
            } => {
                self.statusline.refresh_git_branch();
                let expanded = self.turn.tool_finished(&call_id);
                self.turn
                    .set_activity_phase(if self.turn.tool_calls().is_running() {
                        ActivityPhase::RunningTool
                    } else {
                        ActivityPhase::Starting
                    });
                let image =
                    image_asset
                        .as_ref()
                        .and_then(|asset| match self.load_feed_image(asset) {
                            Ok(image) => image,
                            Err(error) => {
                                card.push_fact(rho_tools::tool_card::ToolFact::Error {
                                    text: format!("image preview unavailable: {error}"),
                                });
                                None
                            }
                        });
                Some(Entry::Tool(ToolEntry {
                    card,
                    expanded,
                    image,
                    started_at: None,
                }))
            }
        }
    }

    pub(super) fn push_transcript_entry(&mut self, entry: Entry) {
        match entry {
            Entry::Assistant(text) => {
                let index = if matches!(self.history.last(), Some(Entry::Assistant(_))) {
                    self.history.len().saturating_sub(1)
                } else {
                    self.history.len()
                };
                match self.history.last_mut() {
                    Some(Entry::Assistant(previous)) => {
                        previous.push_str(&text);
                        self.history.lines_mut().entry_appended(index);
                    }
                    _ => {
                        self.history.lines_mut().invalidate_from(index);
                        self.history.push(Entry::Assistant(text));
                    }
                }
                self.mark_markdown_images_dirty_from(index);
            }
            Entry::Reasoning(reasoning) => match self.history.last_mut() {
                Some(Entry::Reasoning(previous)) if previous.thought_for.is_none() => {
                    previous.text.push_str(&reasoning.text);
                    let closes_thought = reasoning.thought_for.is_some();
                    if closes_thought {
                        previous.thought_for = reasoning.thought_for;
                    }
                    let index = self.history.len().saturating_sub(1);
                    if closes_thought {
                        // The thought summary appends a suffix line the
                        // incremental path cannot produce; re-render the entry.
                        self.history.lines_mut().invalidate_from(index);
                    } else {
                        // Streamed reasoning text extends in place like
                        // assistant text; re-rendering the whole entry per
                        // delta made long thoughts quadratic.
                        self.history.lines_mut().entry_appended(index);
                    }
                }
                _ => {
                    let index = self.history.len();
                    self.history.lines_mut().invalidate_from(index);
                    self.history.push(Entry::Reasoning(reasoning));
                }
            },
            other => {
                let index = self.history.len();
                self.history.lines_mut().invalidate_from(index);
                self.history.push(other);
            }
        }
    }
    pub(super) fn finish_streams(&mut self) -> bool {
        let reasoning_finished = self.finish_stream(StreamKind::Reasoning);
        let assistant_finished = self.finish_stream(StreamKind::Assistant);
        self.streams.current_stream_kind = None;
        self.streams.clear_tick_deadline();
        self.streams.live_stream_preview = None;
        let thought = self.close_reasoning_phase();
        reasoning_finished || assistant_finished || thought
    }

    /// Ends the current reasoning stretch, attaching or inserting a thought duration.
    pub(super) fn close_reasoning_phase(&mut self) -> bool {
        let Some(elapsed) = self.turn.reasoning_phase_mut().finalize() else {
            return false;
        };
        match self.history.last_mut() {
            Some(Entry::Reasoning(reasoning)) if reasoning.thought_for.is_none() => {
                reasoning.thought_for = Some(elapsed);
                let index = self.history.len().saturating_sub(1);
                self.history.lines_mut().invalidate_from(index);
                true
            }
            _ => {
                self.insert_entry(&Entry::Reasoning(ReasoningEntry::summary_only(elapsed)));
                true
            }
        }
    }

    pub(super) fn finish_stream(&mut self, kind: StreamKind) -> bool {
        if self.streams.current_stream_kind == Some(kind) {
            self.streams.flush_hold(kind);
        }
        let fragment = self.streams.stream_mut(kind).finish();
        self.streams.clear_tick_deadline();
        if let Some(fragment) = fragment {
            self.streams.live_stream_preview = None;
            self.insert_stream_fragment(fragment, kind);
            true
        } else {
            false
        }
    }

    pub(super) fn insert_assistant_images(&mut self, content: &[ContentBlock]) {
        for block in content {
            let ContentBlock::Image(image) = block else {
                continue;
            };
            let preview =
                super::feed_image::preview_generated_image(image, self.image_picker.as_ref());
            self.insert_entry(&super::message_history::generated_image_entry(
                preview, image,
            ));
        }
    }

    pub(super) fn insert_final_answer_suffix(&mut self, answer: &str) {
        match final_answer_delta(self.streams.assistant_stream.emitted_text(), answer) {
            FinalAnswerDelta::None => {}
            FinalAnswerDelta::Append(suffix) => {
                self.streams.assistant_stream.push_delta(suffix);
                if let Some(fragment) = self.streams.assistant_stream.finish() {
                    self.insert_stream_fragment(fragment, StreamKind::Assistant);
                }
            }
            FinalAnswerDelta::Mismatch => {
                self.replace_current_turn_assistant_transcript(answer);
            }
        }
    }

    pub(super) fn insert_stream_fragment(&mut self, fragment: StreamFragment, kind: StreamKind) {
        let render_text = fragment.render_text();
        if !render_text.is_empty() {
            update_code_block_state(render_text, self.streams.code_fence_mut(kind));
        }
        let text = fragment.into_text();
        self.push_transcript_entry(kind.entry(text));
    }

    pub(super) fn replace_current_turn_assistant_transcript(&mut self, answer: &str) {
        let start = self.turn.current_turn_start().unwrap_or(0);
        let assistant_indices = self
            .history
            .entries()
            .iter()
            .enumerate()
            .skip(start)
            .filter_map(|(index, entry)| matches!(entry, Entry::Assistant(_)).then_some(index))
            .collect::<Vec<_>>();

        let Some((first, stale)) = assistant_indices.split_first() else {
            self.push_transcript_entry(Entry::Assistant(answer.to_string()));
            return;
        };

        if let Entry::Assistant(text) = &mut self.history.entries_mut()[*first] {
            *text = answer.to_string();
        }
        self.history.images_mut().clear();
        self.history.invalidate_from(*first);
        for index in stale.iter().rev() {
            self.history.entries_mut().remove(*index);
        }
    }

    pub(super) fn insert_entry(&mut self, entry: &Entry) {
        self.record_inserted_entry(entry.clone());
    }

    /// Show ephemeral status feedback.
    ///
    /// Always records the latest status text. Actionable feedback also opens a
    /// short-lived top-right toast; routine mode labels and progress (for
    /// example `ready`, `running`, `running step N`) stay silent so they do not
    /// spam the corner.
    pub(super) fn set_status(&mut self, status: impl AsRef<str>) {
        self.write_status(status.as_ref(), /*allow_toast*/ true);
    }

    /// Record status without a toast. Use for picker titles and other mode
    /// labels that are already visible in the composer UI.
    pub(super) fn set_status_quiet(&mut self, status: impl AsRef<str>) {
        self.write_status(status.as_ref(), /*allow_toast*/ false);
    }

    /// Put up the MCP connect indicator and record that it is ours, so the
    /// hydrate poll can retire it later without matching on its wording.
    pub(super) fn set_mcp_connecting_status(&mut self) {
        self.set_status("connecting MCP servers");
        self.status_source = super::StatusSource::McpConnecting;
    }

    fn write_status(&mut self, status: &str, allow_toast: bool) {
        // Any other status takes ownership, including a clear.
        self.status_source = super::StatusSource::Other;
        if status.is_empty() {
            self.last_status.clear();
            self.status_overlay = None;
            return;
        }
        self.last_status = status.to_string();
        if allow_toast && super::status_overlay::should_toast(status) {
            self.status_overlay = Some(super::status_overlay::StatusOverlay::new(
                status,
                super::status_overlay::tone_for_message(status),
                Instant::now(),
            ));
        } else {
            // Drop a prior toast when entering a silent mode label.
            self.status_overlay = None;
        }
    }

    /// Latest status text, or empty when none is set.
    pub(super) fn status(&self) -> &str {
        &self.last_status
    }

    /// Show ephemeral feedback as a status toast only (no transcript notice).
    pub(super) fn notify_status(&mut self, status: impl AsRef<str>) {
        self.set_status(status);
    }

    pub(super) fn record_inserted_entry(&mut self, entry: Entry) {
        self.push_transcript_entry(entry);
    }

    /// Apply the live transcript chrome settings to in-flight turn UI.
    ///
    /// `Thinking...` visibility is decided at render from
    /// [`crate::tui::ReasoningChrome`] + whether the reasoning stretch is open.
    /// This only drops an in-flight reasoning text preview when policy no longer
    /// wants full text.
    pub(super) fn apply_reasoning_output_visibility(&mut self) {
        if !self.info.runtime.displays_reasoning_output() {
            self.discard_live_reasoning_output();
        }
    }

    pub(super) fn discard_live_reasoning_output(&mut self) {
        let clearing_reasoning = matches!(
            self.streams.current_stream_kind,
            Some(StreamKind::Reasoning)
        ) || self
            .streams
            .live_stream_preview
            .as_ref()
            .is_some_and(|preview| preview.kind == StreamKind::Reasoning);
        if !clearing_reasoning {
            return;
        }
        if matches!(
            self.streams.current_stream_kind,
            Some(StreamKind::Reasoning)
        ) {
            self.streams.discard_hold();
            self.streams.reasoning_stream.reset();
            self.streams.reasoning_stream_code_fence = Default::default();
            self.streams.current_stream_kind = None;
        }
        self.streams.clear_tick_deadline();
        self.streams.live_stream_preview = None;
    }

    fn reset_attempt_accounting(&mut self) {
        self.usage.usage_cost_tracker.attempt_restarted();
        self.usage.run_usage.attempt_reset();
    }

    fn reset_provider_attempt_stream(&mut self, retry: super::activity::ProviderRetryHint) {
        self.reset_streams();
        self.turn.clear_tool_calls();
        if let Some(start) = self
            .turn
            .provider_attempt_mut()
            .reset_output(self.history.entries_mut())
        {
            self.history.images_mut().clear();
            self.history.invalidate_from(start);
        }
        self.set_status(retry.status_label());
        self.turn.set_provider_retry(retry);
    }
}

#[cfg(test)]
#[path = "transcript_events_tests.rs"]
mod tests;
