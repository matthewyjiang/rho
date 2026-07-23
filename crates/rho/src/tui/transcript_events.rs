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
    event_adapter::{self, ViewModelEvent},
    markdown::{update_code_block_state, CodeFenceState},
    render::padded_content_width,
    stream::StreamFragment,
    tool_output_ui::is_tool_entry,
    usage_cost::{
        add_optional, merge_usage, usage_difference, usage_with_estimated_cost, CostSource,
    },
    App, Entry, FinalAnswerDelta, LiveStreamPreview, ReasoningEntry, StreamKind, ToolEntry,
    ToolEntryState, STREAM_PREVIEW_DELAY, STREAM_PREVIEW_MIN_CHARS,
};

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
        | ViewModelEvent::ToolStarted { .. }
        | ViewModelEvent::ToolFinished { .. } => true,
        ViewModelEvent::RunStarted
        | ViewModelEvent::SteeringApplied(_)
        | ViewModelEvent::ProviderStreamReset
        | ViewModelEvent::ProviderRetry
        | ViewModelEvent::CompactionStarted
        | ViewModelEvent::CompactionCompleted { .. }
        | ViewModelEvent::OutputDelta(_)
        | ViewModelEvent::ReasoningDelta(_)
        | ViewModelEvent::ContextUsage(_)
        | ViewModelEvent::Usage(_)
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
            ViewModelEvent::ProviderStreamReset => {
                self.reset_provider_attempt_stream();
                Ok(true)
            }
            ViewModelEvent::OutputDelta(text) => {
                let switched = self.switch_stream_kind(StreamKind::Assistant);
                self.streams.assistant_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Assistant)?;
                self.update_stream_preview_deadline(StreamKind::Assistant);
                Ok(switched || drained)
            }
            ViewModelEvent::ReasoningDelta(text) => {
                let show_reasoning = self.info.runtime.show_reasoning_output;
                self.turn
                    .reasoning_phase_mut()
                    .on_reasoning_delta(show_reasoning);
                if !show_reasoning {
                    return Ok(true);
                }
                let switched = self.switch_stream_kind(StreamKind::Reasoning);
                self.streams.reasoning_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Reasoning)?;
                self.update_stream_preview_deadline(StreamKind::Reasoning);
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
        self.update_stream_preview_deadline(kind);
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

    pub(super) fn drain_stream<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        kind: StreamKind,
    ) -> Result<bool, B::Error> {
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let fragment = match kind {
            StreamKind::Assistant => self.streams.assistant_stream.drain_renderable_markdown(
                inner_width,
                self.streams.assistant_stream_code_fence.is_open(),
            ),
            StreamKind::Reasoning => self.streams.reasoning_stream.drain_renderable_markdown(
                inner_width,
                self.streams.reasoning_stream_code_fence.is_open(),
            ),
        };
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

    pub(super) fn drain_stream_preview(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<bool> {
        if self
            .streams
            .stream_preview_deadline
            .is_none_or(|deadline| Instant::now() < deadline)
        {
            return Ok(false);
        }
        let Some(kind) = self.streams.current_stream_kind else {
            self.streams.stream_preview_deadline = None;
            return Ok(false);
        };
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let preview = match kind {
            StreamKind::Assistant => self.streams.assistant_stream.drain_preview_markdown(
                inner_width,
                self.streams.assistant_stream_code_fence.is_open(),
            ),
            StreamKind::Reasoning => self.streams.reasoning_stream.drain_preview_markdown(
                inner_width,
                self.streams.reasoning_stream_code_fence.is_open(),
            ),
        };
        self.streams.stream_preview_deadline = None;
        self.update_stream_preview_deadline(kind);
        if let Some(preview) = preview {
            self.streams.live_stream_preview = Some(LiveStreamPreview {
                kind,
                text: preview.render_text().to_string(),
                include_leading_blank: preview.include_leading_blank(),
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn record_agent_event(&mut self, event: ViewModelEvent) -> Option<Entry> {
        match event {
            ViewModelEvent::RunStarted => {
                self.usage.usage_cost_tracker.run_started();
                self.usage.usage_before_current_run = self.usage.cumulative_usage.clone();
                self.usage.usage_before_current_step = None;
                self.usage.usage_before_current_attempt = None;
                self.usage.current_run_usage = None;
                None
            }
            ViewModelEvent::StepStarted(step) => {
                self.usage.usage_cost_tracker.step_started();
                self.usage.usage_before_current_step = self.usage.current_run_usage.clone();
                self.usage.usage_before_current_attempt = None;
                self.reset_streams();
                self.turn.provider_attempt_mut().begin(self.history.len());
                self.turn
                    .reasoning_phase_mut()
                    .begin_step(self.info.runtime.show_reasoning_output);
                self.begin_provider_turn_ui();
                self.turn.tool_calls_mut().clear();
                self.turn.loading_spinner_mut().start_if_needed();
                self.status = format!("running step {step}");
                None
            }
            ViewModelEvent::SteeringApplied(ids) => {
                self.mark_steering_applied(&ids);
                None
            }
            ViewModelEvent::ToolStarted {
                call_id,
                display_lines,
            } => {
                self.turn.tool_calls_mut().started(call_id, display_lines);
                None
            }
            ViewModelEvent::ToolUpdated {
                call_id,
                display_lines,
            } => {
                self.turn.tool_calls_mut().updated(call_id, display_lines);
                None
            }
            ViewModelEvent::ToolCallUpdated {
                index,
                call_id,
                display_lines,
            } => {
                self.turn
                    .tool_calls_mut()
                    .preview(index, call_id, display_lines);
                None
            }
            ViewModelEvent::ProviderStreamReset | ViewModelEvent::ProviderRetry => {
                self.usage.usage_cost_tracker.attempt_restarted();
                self.usage.usage_before_current_attempt =
                    self.usage.current_run_usage.as_ref().map(|usage| {
                        usage_difference(usage, self.usage.usage_before_current_step.as_ref())
                    });
                None
            }
            ViewModelEvent::OutputDelta(_) | ViewModelEvent::ReasoningDelta(_) => None,
            ViewModelEvent::CompactionStarted => Some(Entry::Notice(
                event_adapter::COMPACTION_STARTED_NOTICE.into(),
            )),
            ViewModelEvent::CompactionCompleted {
                previous_messages,
                current_messages,
            } => Some(Entry::Notice(event_adapter::compaction_completed_notice(
                previous_messages,
                current_messages,
            ))),
            ViewModelEvent::ContextUsage(usage) => {
                self.info.services.diagnostics.record_context(usage.clone());
                self.usage.current_context = Some(usage);
                None
            }
            ViewModelEvent::Usage(usage) => {
                let current_cost_source = self.usage.usage_cost_tracker.record_usage(&usage);
                let mut current_run_usage = usage;
                if let Some(attempt_baseline) = &self.usage.usage_before_current_attempt {
                    current_run_usage =
                        usage_with_estimated_cost(current_run_usage, self.model_metadata.as_ref());
                    let mut combined = None;
                    merge_usage(&mut combined, attempt_baseline.clone());
                    merge_usage(&mut combined, current_run_usage);
                    current_run_usage = combined.expect("attempt baseline is present");
                }
                let step_baseline =
                    self.usage.usage_before_current_step.clone().map(|usage| {
                        usage_with_estimated_cost(usage, self.model_metadata.as_ref())
                    });
                let mut latest_usage = usage_difference(&current_run_usage, step_baseline.as_ref());
                latest_usage =
                    usage_with_estimated_cost(latest_usage, self.model_metadata.as_ref());
                if current_cost_source == CostSource::Estimated {
                    current_run_usage.cost_usd_micros = add_optional(
                        step_baseline
                            .as_ref()
                            .and_then(|usage| usage.cost_usd_micros),
                        latest_usage.cost_usd_micros,
                    );
                }
                self.usage.current_run_usage = Some(current_run_usage.clone());
                self.usage.latest_usage = Some(latest_usage);
                self.usage
                    .cumulative_usage
                    .clone_from(&self.usage.usage_before_current_run);
                merge_usage(&mut self.usage.cumulative_usage, current_run_usage);
                None
            }
            ViewModelEvent::ToolFinished {
                call_id,
                ok,
                display_style,
                mut display_lines,
                image_asset,
            } => {
                self.statusline.refresh_git_branch();
                let expanded = self.turn.tool_calls_mut().finished(&call_id);
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
                                display_lines.push(format!("image preview unavailable: {error}"));
                                None
                            }
                        });
                Some(Entry::Tool(ToolEntry {
                    state: ToolEntryState::Finished { ok, display_style },
                    display_lines,
                    expanded,
                    image,
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
                        self.history.lines_mut().assistant_appended(index);
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
                    if reasoning.thought_for.is_some() {
                        previous.thought_for = reasoning.thought_for;
                    }
                    let index = self.history.len().saturating_sub(1);
                    self.history.lines_mut().invalidate_from(index);
                }
                _ => {
                    let index = self.history.len();
                    self.history.lines_mut().invalidate_from(index);
                    self.history.push(Entry::Reasoning(reasoning));
                }
            },
            other => {
                self.history.set_last_status_notice(match &other {
                    Entry::Notice(text) => Some(text.clone()),
                    _ => None,
                });
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
        self.streams.stream_preview_deadline = None;
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
        let fragment = match kind {
            StreamKind::Assistant => self.streams.assistant_stream.finish(),
            StreamKind::Reasoning => self.streams.reasoning_stream.finish(),
        };
        self.update_stream_preview_deadline(kind);
        if let Some(fragment) = fragment {
            self.streams.live_stream_preview = None;
            self.insert_stream_fragment(fragment, kind);
            true
        } else {
            false
        }
    }

    pub(super) fn update_stream_preview_deadline(&mut self, kind: StreamKind) {
        let pending_chars = match kind {
            StreamKind::Assistant => self.streams.assistant_stream.pending_text().chars().count(),
            StreamKind::Reasoning => self.streams.reasoning_stream.pending_text().chars().count(),
        };
        if pending_chars < STREAM_PREVIEW_MIN_CHARS {
            self.streams.stream_preview_deadline = None;
        } else if self.streams.stream_preview_deadline.is_none() {
            self.streams.stream_preview_deadline = Some(Instant::now() + STREAM_PREVIEW_DELAY);
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
            let code_fence = match kind {
                StreamKind::Assistant => &mut self.streams.assistant_stream_code_fence,
                StreamKind::Reasoning => &mut self.streams.reasoning_stream_code_fence,
            };
            update_code_block_state(render_text, code_fence);
            self.history.set_last_inserted_was_tool(false);
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

    pub(super) fn notify_status(&mut self, status: impl Into<String>) {
        let status = status.into();
        self.status = status.clone();
        if self.history.last_status_notice() == Some(status.as_str()) {
            return;
        }
        self.insert_entry(&Entry::Notice(status));
    }

    pub(super) fn record_inserted_entry(&mut self, entry: Entry) {
        self.history.set_last_status_notice(match &entry {
            Entry::Notice(text) => Some(text.clone()),
            Entry::User(_)
            | Entry::Assistant(_)
            | Entry::Reasoning(_)
            | Entry::RuntimeInfo(_)
            | Entry::UsageLimits(_)
            | Entry::Tool(_)
            | Entry::Error(_) => None,
        });
        self.history
            .set_last_inserted_was_tool(is_tool_entry(&entry));
        self.push_transcript_entry(entry);
    }

    /// Apply the live `show_reasoning_output` setting to in-flight turn UI.
    pub(super) fn apply_reasoning_output_visibility(&mut self) {
        if self.info.runtime.show_reasoning_output {
            self.turn
                .reasoning_phase_mut()
                .set_hidden_placeholder(false);
            return;
        }

        self.discard_live_reasoning_output();

        // Keep the Thinking... placeholder while this step is still waiting for
        // or streaming reasoning. Later phases (response, tools) stay clear.
        let hide_placeholder = self.is_ui_busy()
            && (self.turn.reasoning_phase().has_started()
                || matches!(
                    self.turn.activity_phase(),
                    ActivityPhase::Starting
                        | ActivityPhase::WaitingForProvider
                        | ActivityPhase::Thinking
                        | ActivityPhase::RetryingProvider
                ));
        self.turn
            .reasoning_phase_mut()
            .set_hidden_placeholder(hide_placeholder);
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
            self.streams.reasoning_stream.reset();
            self.streams.reasoning_stream_code_fence = CodeFenceState::default();
            self.streams.current_stream_kind = None;
        }
        self.streams.stream_preview_deadline = None;
        self.streams.live_stream_preview = None;
    }

    pub(super) fn reset_provider_attempt_stream(&mut self) {
        self.reset_streams();
        self.turn.tool_calls_mut().clear();
        if let Some(start) = self
            .turn
            .provider_attempt_mut()
            .reset_output(self.history.entries_mut())
        {
            self.history.images_mut().clear();
            self.history.invalidate_from(start);
        }
        self.status = "retrying provider response".into();
    }
}

#[cfg(test)]
#[path = "transcript_events_tests.rs"]
mod tests;
