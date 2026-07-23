//! Live provider stream and transcript event handling for the interactive TUI.
//!
//! This module owns App methods that switch and drain assistant/reasoning
//! streams, schedule stream previews, record usage and cost from view-model
//! events, drive tool-call lifecycle display state, and merge finished text
//! into the transcript. Stream finalization that must happen before recording
//! a lifecycle event is classified exhaustively via
//! [`should_finish_streams_before_recording`].

use std::time::Instant;

use ratatui::{backend::Backend, DefaultTerminal, Terminal};

use super::{
    activity::ActivityPhase,
    add_optional,
    event_adapter::{self, ViewModelEvent},
    markdown::CodeFenceState,
    merge_usage, padded_content_width,
    usage_cost::CostSource,
    usage_difference, usage_with_estimated_cost, App, Entry, LiveStreamPreview, StreamKind,
    ToolEntry, ToolEntryState,
};

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
        self.assistant_stream.reset();
        self.assistant_stream_code_fence = CodeFenceState::default();
        self.reasoning_stream.reset();
        self.reasoning_stream_code_fence = CodeFenceState::default();
        self.current_stream_kind = None;
        self.stream_preview_deadline = None;
        self.live_stream_preview = None;
        // Discard an unfinished reasoning phase. Callers that should keep a
        // summary must finalize before reset (for example `finish_streams`).
        self.reasoning_phase.reset();
    }

    pub(super) fn handle_agent_event<B: Backend>(
        &mut self,
        event: ViewModelEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<bool, B::Error> {
        if let Some(phase) = event.activity_phase() {
            self.activity_phase = phase;
        }
        match event {
            ViewModelEvent::ProviderStreamReset => {
                self.reset_provider_attempt_stream();
                Ok(true)
            }
            ViewModelEvent::OutputDelta(text) => {
                let switched = self.switch_stream_kind(StreamKind::Assistant);
                self.assistant_stream.push_delta(&text);
                let drained = self.drain_stream(terminal, StreamKind::Assistant)?;
                self.update_stream_preview_deadline(StreamKind::Assistant);
                Ok(switched || drained)
            }
            ViewModelEvent::ReasoningDelta(text) => {
                let show_reasoning = self.info.runtime.show_reasoning_output;
                self.reasoning_phase.on_reasoning_delta(show_reasoning);
                if !show_reasoning {
                    return Ok(true);
                }
                let switched = self.switch_stream_kind(StreamKind::Reasoning);
                self.reasoning_stream.push_delta(&text);
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
            && self.current_stream_kind != Some(StreamKind::Assistant)
        {
            self.close_reasoning_phase()
        } else {
            false
        };
        self.current_stream_kind = Some(kind);
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
            StreamKind::Assistant => self
                .assistant_stream
                .drain_renderable_markdown(inner_width, self.assistant_stream_code_fence.is_open()),
            StreamKind::Reasoning => self
                .reasoning_stream
                .drain_renderable_markdown(inner_width, self.reasoning_stream_code_fence.is_open()),
        };
        if let Some(fragment) = fragment {
            self.live_stream_preview = None;
            self.insert_stream_fragment(fragment, kind);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn finish_current_stream(&mut self) -> bool {
        self.current_stream_kind
            .is_some_and(|kind| self.finish_stream(kind))
    }

    pub(super) fn drain_stream_preview(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<bool> {
        if self
            .stream_preview_deadline
            .is_none_or(|deadline| Instant::now() < deadline)
        {
            return Ok(false);
        }
        let Some(kind) = self.current_stream_kind else {
            self.stream_preview_deadline = None;
            return Ok(false);
        };
        let width = terminal.size()?.width as usize;
        let inner_width = padded_content_width(width);
        let preview = match kind {
            StreamKind::Assistant => self
                .assistant_stream
                .drain_preview_markdown(inner_width, self.assistant_stream_code_fence.is_open()),
            StreamKind::Reasoning => self
                .reasoning_stream
                .drain_preview_markdown(inner_width, self.reasoning_stream_code_fence.is_open()),
        };
        self.stream_preview_deadline = None;
        self.update_stream_preview_deadline(kind);
        if let Some(preview) = preview {
            self.live_stream_preview = Some(LiveStreamPreview {
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
                self.usage_cost_tracker.run_started();
                self.usage_before_current_run = self.cumulative_usage.clone();
                self.usage_before_current_step = None;
                self.usage_before_current_attempt = None;
                self.current_run_usage = None;
                None
            }
            ViewModelEvent::StepStarted(step) => {
                self.usage_cost_tracker.step_started();
                self.usage_before_current_step = self.current_run_usage.clone();
                self.usage_before_current_attempt = None;
                self.reset_streams();
                self.provider_attempt.begin(self.transcript.len());
                self.reasoning_phase
                    .begin_step(self.info.runtime.show_reasoning_output);
                self.running = true;
                self.tool_calls.clear();
                self.loading_spinner.start_if_needed();
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
                self.tool_calls.started(call_id, display_lines);
                None
            }
            ViewModelEvent::ToolUpdated {
                call_id,
                display_lines,
            } => {
                self.tool_calls.updated(call_id, display_lines);
                None
            }
            ViewModelEvent::ToolCallUpdated {
                index,
                call_id,
                display_lines,
            } => {
                self.tool_calls.preview(index, call_id, display_lines);
                None
            }
            ViewModelEvent::ProviderStreamReset | ViewModelEvent::ProviderRetry => {
                self.usage_cost_tracker.attempt_restarted();
                self.usage_before_current_attempt = self
                    .current_run_usage
                    .as_ref()
                    .map(|usage| usage_difference(usage, self.usage_before_current_step.as_ref()));
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
                self.current_context = Some(usage);
                None
            }
            ViewModelEvent::Usage(usage) => {
                let current_cost_source = self.usage_cost_tracker.record_usage(&usage);
                let mut current_run_usage = usage;
                if let Some(attempt_baseline) = &self.usage_before_current_attempt {
                    current_run_usage =
                        usage_with_estimated_cost(current_run_usage, self.model_metadata.as_ref());
                    let mut combined = None;
                    merge_usage(&mut combined, attempt_baseline.clone());
                    merge_usage(&mut combined, current_run_usage);
                    current_run_usage = combined.expect("attempt baseline is present");
                }
                let step_baseline = self
                    .usage_before_current_step
                    .clone()
                    .map(|usage| usage_with_estimated_cost(usage, self.model_metadata.as_ref()));
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
                self.current_run_usage = Some(current_run_usage.clone());
                self.latest_usage = Some(latest_usage);
                self.cumulative_usage
                    .clone_from(&self.usage_before_current_run);
                merge_usage(&mut self.cumulative_usage, current_run_usage);
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
                let expanded = self.tool_calls.finished(&call_id);
                self.activity_phase = if self.tool_calls.is_running() {
                    ActivityPhase::RunningTool
                } else {
                    ActivityPhase::Starting
                };
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
                let index = if matches!(self.transcript.last(), Some(Entry::Assistant(_))) {
                    self.transcript.len().saturating_sub(1)
                } else {
                    self.transcript.len()
                };
                match self.transcript.last_mut() {
                    Some(Entry::Assistant(previous)) => {
                        previous.push_str(&text);
                        self.history_lines.assistant_appended(index);
                    }
                    _ => {
                        self.history_lines.invalidate_from(index);
                        self.transcript.push(Entry::Assistant(text));
                    }
                }
                self.mark_markdown_images_dirty_from(index);
            }
            Entry::Reasoning(reasoning) => match self.transcript.last_mut() {
                Some(Entry::Reasoning(previous)) if previous.thought_for.is_none() => {
                    previous.text.push_str(&reasoning.text);
                    if reasoning.thought_for.is_some() {
                        previous.thought_for = reasoning.thought_for;
                    }
                    self.history_lines
                        .invalidate_from(self.transcript.len().saturating_sub(1));
                }
                _ => {
                    self.history_lines.invalidate_from(self.transcript.len());
                    self.transcript.push(Entry::Reasoning(reasoning));
                }
            },
            other => {
                self.last_status_notice = match &other {
                    Entry::Notice(text) => Some(text.clone()),
                    _ => None,
                };
                self.history_lines.invalidate_from(self.transcript.len());
                self.transcript.push(other);
            }
        }
    }
}

#[cfg(test)]
#[path = "transcript_events_tests.rs"]
mod tests;
