//! Ask before provider-native context is omitted on model switch or resume.
//!
//! Compaction can salvage portable text. It does not make native blocks sendable
//! to an incompatible model.

use crate::{credential_store::build_provider, session::Session};
use ratatui::DefaultTerminal;
use rho_providers::model::{catalog::ModelSelection, Message};
use rho_sdk::model::handoff::HandoffReport;
use rho_sdk::model::ModelIdentity;

use super::{
    catalog, is_tool_entry, recovered_history_tail, short_session_id,
    transcript_entries_from_messages, App, ComposerMode, Entry, InlineChoice,
    InlineChoiceKeyOutcome, InlineChoiceOption, InteractiveModelSelection, InteractiveRuntime,
    UiPicker, RECOVERED_HISTORY_LINE_LIMIT,
};

pub(super) const ACTION_USE_SOURCE: &str = "use-source";
pub(super) const ACTION_COMPACT: &str = "compact";
pub(super) const ACTION_CONTINUE: &str = "continue";

#[derive(Debug)]
pub(super) struct ModelHandoffChoice {
    pub(super) choice: InlineChoice,
    pending: PendingContextHandoff,
    pub(super) return_picker: Option<(Box<UiPicker>, String)>,
}

#[derive(Debug)]
enum PendingContextHandoff {
    ModelSwitch {
        selection: InteractiveModelSelection,
        continuation: HandoffContinuation,
    },
    Resume {
        session: Session,
        model_history: Vec<Message>,
        display_history: Vec<Message>,
        source_selection: Option<InteractiveModelSelection>,
    },
    LoadedSession {
        source_selection: Option<InteractiveModelSelection>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandoffContinuation {
    None,
    PendingTurnWork,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OmissionSurface {
    Notice,
    Silent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextHandoffKind {
    ModelSwitch,
    Resume,
    LoadedSession,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContextHandoffImpact {
    source_label: String,
    target_label: String,
    omissions: HandoffReport,
    can_compact: bool,
    source_model_available: bool,
    cache_warm: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextHandoffDecision {
    UseSourceModel,
    CompactThenContinue,
    ContinueDirect,
}

impl ContextHandoffImpact {
    fn should_prompt(&self) -> bool {
        self.omissions.has_omissions() || (self.cache_warm && self.can_compact)
    }

    fn choice(&self, kind: ContextHandoffKind) -> anyhow::Result<InlineChoice> {
        let title = match kind {
            ContextHandoffKind::ModelSwitch => {
                format!("How should Rho switch to {}?", self.target_label)
            }
            ContextHandoffKind::Resume => {
                format!("How should Rho resume on {}?", self.target_label)
            }
            ContextHandoffKind::LoadedSession => {
                format!("How should Rho continue on {}?", self.target_label)
            }
        };
        InlineChoice::new(title, self.description(kind), self.options(kind)?)
    }

    fn description(&self, kind: ContextHandoffKind) -> String {
        if self.omissions.has_omissions() {
            let kinds = self.omissions.omitted_kinds.join(", ");
            let blocks = self.omissions.omitted_provider_context;
            let native = format!(
                "This conversation has {blocks} provider-native context block(s) ({kinds}). {} cannot use them.",
                self.target_label
            );
            match kind {
                ContextHandoffKind::ModelSwitch | ContextHandoffKind::Resume => format!(
                    "{native} Compaction can summarize portable context first; it does not make native blocks sendable."
                ),
                ContextHandoffKind::LoadedSession => format!(
                    "{native} The transcript is already loaded. Compaction can summarize portable context first; it does not make native blocks sendable."
                ),
            }
        } else {
            format!(
                "Compacting uses {} and may reduce the context sent to {}.",
                self.source_label, self.target_label
            )
        }
    }

    fn options(&self, kind: ContextHandoffKind) -> anyhow::Result<Vec<InlineChoiceOption>> {
        let mut options = Vec::new();
        let mut shortcut = '1';

        if matches!(
            kind,
            ContextHandoffKind::Resume | ContextHandoffKind::LoadedSession
        ) && self.source_model_available
        {
            let label: String = match kind {
                ContextHandoffKind::LoadedSession => format!("Switch to {}", self.source_label),
                _ => format!("Resume with {}", self.source_label),
            };
            let detail: String = match kind {
                ContextHandoffKind::LoadedSession => {
                    "Use the model that produced the native context so it can be replayed.".into()
                }
                _ => "Keep provider-native context for the model that produced it.".into(),
            };
            options.push(InlineChoiceOption::available(
                ACTION_USE_SOURCE,
                shortcut,
                label,
                detail,
            ));
            shortcut = next_shortcut(shortcut);
        }

        if self.can_compact {
            let detail = if self.omissions.has_omissions() {
                format!(
                    "Summarize with {}, then continue. {} native block(s) still will not be sent to {}.",
                    self.source_label,
                    self.omissions.omitted_provider_context,
                    self.target_label
                )
            } else {
                format!("Compact with {}, then continue.", self.source_label)
            };
            let label = if self.omissions.has_omissions() {
                "Compact, then continue"
            } else {
                "Compact and switch"
            };
            options.push(InlineChoiceOption::available(
                ACTION_COMPACT,
                shortcut,
                label,
                detail,
            ));
            shortcut = next_shortcut(shortcut);
        }

        let continue_label = match kind {
            ContextHandoffKind::ModelSwitch => {
                if self.omissions.has_omissions() {
                    "Switch now".into()
                } else {
                    "Switch directly".into()
                }
            }
            ContextHandoffKind::Resume => format!("Resume with {}", self.target_label),
            ContextHandoffKind::LoadedSession => format!("Continue with {}", self.target_label),
        };
        let continue_detail = if self.omissions.has_omissions() {
            format!(
                "{} native block(s) will not be sent to {}. Transcript and tool history remain.",
                self.omissions.omitted_provider_context, self.target_label
            )
        } else {
            "Keep the full conversation context and continue now.".into()
        };
        options.push(InlineChoiceOption::available(
            ACTION_CONTINUE,
            shortcut,
            continue_label,
            continue_detail,
        ));

        if options.is_empty() {
            anyhow::bail!("context handoff has no available options");
        }
        Ok(options)
    }
}

impl App {
    pub(super) fn request_model_selection(
        &mut self,
        selection: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.prepare_model_selection(selection, HandoffContinuation::None, agent)
    }

    pub(super) fn request_model_selection_after_turn(
        &mut self,
        selection: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.prepare_model_selection(selection, HandoffContinuation::PendingTurnWork, agent)
    }

    fn prepare_model_selection(
        &mut self,
        selection: InteractiveModelSelection,
        continuation: HandoffContinuation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let target = &selection.selection;
        let target_label =
            rho_providers::provider::model_reference(&target.provider, &target.model);
        let source_label = rho_providers::provider::model_reference(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        let target_identity =
            match model_identity_for_selection(target, self.info.runtime.reasoning) {
                Ok(identity) => identity,
                Err(_) => {
                    return self.select_model(selection, agent, OmissionSurface::Notice);
                }
            };
        let omissions = agent.provider_context_omissions(&target_identity);
        let impact = ContextHandoffImpact {
            source_label,
            target_label,
            omissions,
            can_compact: agent.can_compact(),
            source_model_available: false,
            cache_warm: self.model_cache_warm,
        };
        if !impact.should_prompt()
            || (target.provider == self.info.runtime.provider
                && target.model == self.info.runtime.model
                && target.auth == self.info.runtime.auth)
        {
            return self.select_model(selection, agent, OmissionSurface::Notice);
        }

        let choice = impact.choice(ContextHandoffKind::ModelSwitch)?;
        self.composer = ComposerMode::ModelHandoffChoice(ModelHandoffChoice {
            choice,
            pending: PendingContextHandoff::ModelSwitch {
                selection,
                continuation,
            },
            return_picker: None,
        });
        self.status = "choose model handoff".into();
        Ok(())
    }

    pub(super) fn offer_resume_context_handoff(
        &mut self,
        session: Session,
        model_history: Vec<Message>,
        display_history: Vec<Message>,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let target_identity = agent.provider_identity();
        let omissions = InteractiveRuntime::provider_context_omissions_for_messages(
            &model_history,
            &target_identity,
        );
        if !omissions.has_omissions() {
            return Ok(false);
        }

        let source_identity = session
            .stored_provider_identity()?
            .or_else(|| first_omitted_source_identity(&model_history, &target_identity));
        let source_selection = source_identity
            .as_ref()
            .and_then(|identity| self.selection_for_identity(identity));
        let source_label = source_identity
            .as_ref()
            .map(|identity| {
                rho_providers::provider::model_reference(&identity.provider, &identity.model)
            })
            .unwrap_or_else(|| "session model".into());
        let target_label = rho_providers::provider::model_reference(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        let impact = ContextHandoffImpact {
            source_label,
            target_label,
            omissions,
            can_compact: source_selection.is_some() && agent.can_compact_messages(&model_history),
            source_model_available: source_selection.is_some(),
            cache_warm: false,
        };
        if !impact.should_prompt() {
            return Ok(false);
        }

        let choice = impact.choice(ContextHandoffKind::Resume)?;
        self.composer = ComposerMode::ModelHandoffChoice(ModelHandoffChoice {
            choice,
            pending: PendingContextHandoff::Resume {
                session,
                model_history,
                display_history,
                source_selection,
            },
            return_picker: None,
        });
        self.status = "choose resume handoff".into();
        Ok(true)
    }

    pub(super) fn maybe_offer_loaded_session_context_handoff(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if !matches!(self.composer, ComposerMode::Input) {
            return Ok(());
        }
        if self.info.session.recovered_messages.is_empty() {
            self.insert_runtime_notices(agent);
            return Ok(());
        }

        let target_identity = agent.provider_identity();
        let omissions = agent.provider_context_omissions(&target_identity);
        let notices = agent.take_notices();
        for notice in notices {
            if !is_omission_notice(&notice) {
                self.insert_entry(&Entry::Notice(notice));
            }
        }
        if !omissions.has_omissions() {
            return Ok(());
        }

        let source_identity = agent
            .stored_session()
            .and_then(|session| session.stored_provider_identity().ok().flatten())
            .or_else(|| first_omitted_source_identity(&agent.history(), &target_identity));
        let source_selection = source_identity
            .as_ref()
            .and_then(|identity| self.selection_for_identity(identity));
        let source_label = source_identity
            .as_ref()
            .map(|identity| {
                rho_providers::provider::model_reference(&identity.provider, &identity.model)
            })
            .unwrap_or_else(|| "session model".into());
        let target_label = rho_providers::provider::model_reference(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        );
        let impact = ContextHandoffImpact {
            source_label,
            target_label,
            omissions,
            can_compact: source_selection.is_some() && agent.can_compact(),
            source_model_available: source_selection.is_some(),
            cache_warm: false,
        };
        let choice = impact.choice(ContextHandoffKind::LoadedSession)?;
        self.composer = ComposerMode::ModelHandoffChoice(ModelHandoffChoice {
            choice,
            pending: PendingContextHandoff::LoadedSession { source_selection },
            return_picker: None,
        });
        self.status = "choose resume handoff".into();
        Ok(())
    }

    fn selection_for_identity(
        &mut self,
        identity: &ModelIdentity,
    ) -> Option<InteractiveModelSelection> {
        self.refresh_available_auths();
        let selection =
            catalog::resolve_model_selection_for_provider(&identity.provider, &identity.model)
                .ok()?;
        if !self
            .available_auths
            .iter()
            .any(|auth| auth == &selection.auth)
        {
            return None;
        }
        Some(InteractiveModelSelection {
            selection,
            alias: None,
        })
    }

    pub(super) async fn handle_model_handoff_choice_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let outcome = match &mut self.composer {
            ComposerMode::ModelHandoffChoice(choice) => choice.choice.handle_key(key),
            _ => return Ok(false),
        };
        let (resolved, continuation, return_picker) = match outcome {
            InlineChoiceKeyOutcome::Selected(value) => {
                let ComposerMode::ModelHandoffChoice(choice) =
                    std::mem::replace(&mut self.composer, ComposerMode::Input)
                else {
                    unreachable!("model handoff choice checked above");
                };
                let decision =
                    decision_from_value(&value).unwrap_or(ContextHandoffDecision::ContinueDirect);
                let continuation = match &choice.pending {
                    PendingContextHandoff::ModelSwitch { continuation, .. } => *continuation,
                    PendingContextHandoff::Resume { .. }
                    | PendingContextHandoff::LoadedSession { .. } => HandoffContinuation::None,
                };
                if let Err(err) = self
                    .apply_context_handoff_decision(decision, choice.pending, terminal, agent)
                    .await
                {
                    self.insert_entry(&Entry::Error(format!("model handoff failed: {err}")));
                    self.status = "model handoff failed".into();
                }
                (true, continuation, choice.return_picker)
            }
            InlineChoiceKeyOutcome::Cancelled => {
                let ComposerMode::ModelHandoffChoice(choice) =
                    std::mem::replace(&mut self.composer, ComposerMode::Input)
                else {
                    unreachable!("model handoff choice checked above");
                };
                let continuation = match choice.pending {
                    PendingContextHandoff::ModelSwitch { continuation, .. } => {
                        self.status = "model switch cancelled".into();
                        continuation
                    }
                    PendingContextHandoff::Resume { .. } => {
                        self.status = "resume cancelled".into();
                        HandoffContinuation::None
                    }
                    PendingContextHandoff::LoadedSession { .. } => {
                        self.status = "ready".into();
                        HandoffContinuation::None
                    }
                };
                (true, continuation, choice.return_picker)
            }
            InlineChoiceKeyOutcome::Handled => (false, HandoffContinuation::None, None),
        };
        self.paste_burst.clear();
        self.ctrl_c_streak = 0;
        if let Some((picker, selected_value)) = return_picker {
            self.open_main_config_picker(&selected_value, picker.filter)?;
        }
        if resolved && continuation == HandoffContinuation::PendingTurnWork {
            self.continue_after_model_handoff(terminal, agent).await?;
        }
        Ok(true)
    }

    async fn apply_context_handoff_decision(
        &mut self,
        decision: ContextHandoffDecision,
        pending: PendingContextHandoff,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match pending {
            PendingContextHandoff::ModelSwitch {
                selection,
                continuation: _,
            } => match decision {
                ContextHandoffDecision::CompactThenContinue => {
                    match self.execute_compact_command(terminal, agent).await {
                        Ok(true) => {
                            self.select_model(selection, agent, OmissionSurface::Silent)?;
                        }
                        Ok(false) => {
                            self.insert_entry(&Entry::Notice(
                                "model unchanged because context was not compacted".into(),
                            ));
                        }
                        Err(err) => return Err(err),
                    }
                }
                ContextHandoffDecision::ContinueDirect => {
                    self.select_model(selection, agent, OmissionSurface::Silent)?;
                }
                ContextHandoffDecision::UseSourceModel => {
                    anyhow::bail!("use-source is not valid for model switch");
                }
            },
            PendingContextHandoff::Resume {
                session,
                model_history,
                display_history,
                source_selection,
            } => match decision {
                ContextHandoffDecision::UseSourceModel => {
                    let Some(source) = source_selection else {
                        anyhow::bail!("session model is unavailable");
                    };
                    self.select_model(source, agent, OmissionSurface::Silent)?;
                    self.apply_resume_session(
                        session,
                        model_history,
                        display_history,
                        terminal,
                        agent,
                    )
                    .await?;
                }
                ContextHandoffDecision::CompactThenContinue => {
                    let Some(source) = source_selection else {
                        anyhow::bail!("session model is unavailable for compaction");
                    };
                    let original = current_runtime_selection(self);
                    self.select_model(source, agent, OmissionSurface::Silent)?;
                    self.apply_resume_session(
                        session,
                        model_history,
                        display_history,
                        terminal,
                        agent,
                    )
                    .await?;
                    match self.execute_compact_command(terminal, agent).await {
                        Ok(true) => {
                            self.select_model(original, agent, OmissionSurface::Silent)?;
                        }
                        Ok(false) => {
                            self.insert_entry(&Entry::Notice(
                                "kept the session model because context was not compacted".into(),
                            ));
                        }
                        Err(err) => return Err(err),
                    }
                }
                ContextHandoffDecision::ContinueDirect => {
                    self.apply_resume_session(
                        session,
                        model_history,
                        display_history,
                        terminal,
                        agent,
                    )
                    .await?;
                }
            },
            PendingContextHandoff::LoadedSession { source_selection } => match decision {
                ContextHandoffDecision::UseSourceModel => {
                    let Some(source) = source_selection else {
                        anyhow::bail!("session model is unavailable");
                    };
                    self.select_model(source, agent, OmissionSurface::Silent)?;
                    self.status = "ready".into();
                }
                ContextHandoffDecision::CompactThenContinue => {
                    let Some(source) = source_selection else {
                        anyhow::bail!("session model is unavailable for compaction");
                    };
                    let original = current_runtime_selection(self);
                    self.select_model(source, agent, OmissionSurface::Silent)?;
                    match self.execute_compact_command(terminal, agent).await {
                        Ok(true) => {
                            self.select_model(original, agent, OmissionSurface::Silent)?;
                        }
                        Ok(false) => {
                            self.insert_entry(&Entry::Notice(
                                "kept the session model because context was not compacted".into(),
                            ));
                        }
                        Err(err) => return Err(err),
                    }
                }
                ContextHandoffDecision::ContinueDirect => {
                    self.status = "ready".into();
                }
            },
        }
        Ok(())
    }

    pub(super) async fn apply_resume_session(
        &mut self,
        session: Session,
        model_history: Vec<Message>,
        display_history: Vec<Message>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let full_id = session.id().to_string();
        let short_id = short_session_id(&full_id);

        agent.resume(session, model_history).await?;
        for notice in agent.take_notices() {
            if !is_omission_notice(&notice) {
                self.insert_entry(&Entry::Notice(notice));
            }
        }
        self.model_cache_warm = false;
        self.info.session.session_id = Some(full_id);
        self.info.session.recovered_messages = display_history.clone();
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.reset_streams();
        self.running = false;
        self.goal = None;
        self.reset_usage();
        self.current_context = None;
        let entries = transcript_entries_from_messages(&display_history, &self.info.runtime.cwd);
        let width = terminal.size()?.width as usize;
        let (_omitted, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.runtime.max_tool_output_lines,
        );
        self.transcript = visible_entries;
        self.markdown_images.clear();
        self.mark_markdown_images_dirty_from(0);
        self.history_lines.invalidate_from(0);
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        self.insert_entry(&Entry::Notice(format!("resumed session {short_id}")));
        self.status = format!("resumed {short_id}");
        self.info
            .services
            .herdr
            .report_session(self.info.session.session_id.as_deref())
            .await;
        Ok(())
    }

    async fn continue_after_model_handoff(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if let Some(prompt) = self.queued_prompts.pop_front() {
            self.restore_pending_prompt(prompt);
            return self.submit(terminal, agent).await;
        }
        if self.goal.is_some() && !self.should_quit {
            self.continue_goal(terminal, agent, std::collections::VecDeque::new())
                .await?;
        }
        Ok(())
    }
}

pub(super) fn format_omission_kinds(report: &HandoffReport) -> String {
    report.omitted_kinds.join(", ")
}

fn decision_from_value(value: &str) -> Option<ContextHandoffDecision> {
    match value {
        ACTION_USE_SOURCE => Some(ContextHandoffDecision::UseSourceModel),
        ACTION_COMPACT => Some(ContextHandoffDecision::CompactThenContinue),
        ACTION_CONTINUE => Some(ContextHandoffDecision::ContinueDirect),
        _ => None,
    }
}

fn model_identity_for_selection(
    selection: &ModelSelection,
    reasoning: rho_providers::reasoning::ReasoningLevel,
) -> Result<ModelIdentity, rho_providers::model::ModelError> {
    Ok(build_provider(&selection.provider, &selection.model, reasoning)?.identity())
}

fn current_runtime_selection(app: &App) -> InteractiveModelSelection {
    InteractiveModelSelection {
        selection: ModelSelection {
            provider: app.info.runtime.provider.clone(),
            model: app.info.runtime.model.clone(),
            auth: app.info.runtime.auth.clone(),
            from_catalog: true,
        },
        alias: None,
    }
}

fn first_omitted_source_identity(
    messages: &[Message],
    target: &ModelIdentity,
) -> Option<ModelIdentity> {
    for message in messages {
        let blocks = match message {
            Message::EnrichedAssistant(message) => &message.provider_context,
            Message::AbortedAssistant(message) => &message.provider_context,
            Message::System(_)
            | Message::User(_)
            | Message::Assistant(_)
            | Message::ToolResult(_) => continue,
        };
        for block in blocks {
            if !block.is_replayable_to(target) {
                return Some(block.identity.clone());
            }
        }
    }
    None
}

fn is_omission_notice(notice: &str) -> bool {
    notice.contains("omitted")
        && (notice.contains("provider-native") || notice.contains("nonportable provider context"))
}

fn next_shortcut(current: char) -> char {
    char::from_u32(u32::from(current) + 1).unwrap_or(current)
}

#[cfg(test)]
#[path = "context_handoff_tests.rs"]
mod tests;
