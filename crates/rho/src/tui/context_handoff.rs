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
    catalog,
    message_history::{recovered_history_tail, transcript_entries_from_messages},
    session_picker::short_session_id,
    tool_output_ui::is_tool_entry,
    App, ComposerMode, Entry, InlineChoice, InlineChoiceModal, InlineChoiceOption,
    InlineChoicePending, InteractiveModelSelection, InteractiveRuntime, UiPicker,
    RECOVERED_HISTORY_LINE_LIMIT,
};

pub(super) const ACTION_USE_SOURCE: &str = "use-source";
pub(super) const ACTION_COMPACT: &str = "compact";
pub(super) const ACTION_CONTINUE: &str = "continue";

#[derive(Debug)]
pub(super) struct PendingContextHandoff {
    kind: ContextHandoffKind,
    /// Switch here before compact / use-source when not already current.
    source_selection: Option<InteractiveModelSelection>,
    /// Switch here after materialize/compact when not already current.
    target_selection: Option<InteractiveModelSelection>,
    materialize: Option<ResumeMaterialize>,
    after: AfterHandoff,
}

#[derive(Debug)]
struct ResumeMaterialize {
    session: Session,
    model_history: Vec<Message>,
    display_history: Vec<Message>,
}

#[derive(Debug)]
pub(super) enum AfterHandoff {
    None,
    ContinueTurnWork,
    ReopenConfigPicker {
        picker: Box<UiPicker>,
        selected: String,
    },
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
    offer_use_source: bool,
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
        InlineChoice::new(title, self.description(kind), self.options(kind))
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

    fn options(&self, kind: ContextHandoffKind) -> Vec<InlineChoiceOption> {
        let mut options = Vec::new();
        let mut shortcut = '1';

        if self.offer_use_source {
            let (label, detail): (String, String) = match kind {
                ContextHandoffKind::LoadedSession => (
                    format!("Switch to {}", self.source_label),
                    "Use the model that produced the native context so it can be replayed.".into(),
                ),
                _ => (
                    format!("Resume with {}", self.source_label),
                    "Keep provider-native context for the model that produced it.".into(),
                ),
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
        options
    }
}

impl App {
    pub(super) fn request_model_selection(
        &mut self,
        selection: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.prepare_model_selection(selection, AfterHandoff::None, agent)
    }

    pub(super) fn request_model_selection_after_turn(
        &mut self,
        selection: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.prepare_model_selection(selection, AfterHandoff::ContinueTurnWork, agent)
    }

    pub(super) fn request_model_selection_from_config_picker(
        &mut self,
        selection: InteractiveModelSelection,
        picker: UiPicker,
        selected: &'static str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.prepare_model_selection(
            selection,
            AfterHandoff::ReopenConfigPicker {
                picker: Box::new(picker),
                selected: selected.to_string(),
            },
            agent,
        )
    }

    fn prepare_model_selection(
        &mut self,
        selection: InteractiveModelSelection,
        after: AfterHandoff,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let target = &selection.selection;
        let same_model = target.provider == self.info.runtime.provider
            && target.model == self.info.runtime.model
            && target.auth == self.info.runtime.auth;
        if same_model {
            self.select_model(selection, agent)?;
            return self.finish_after_handoff_sync(after);
        }

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
                    self.select_model_with_omission_notice(selection, agent)?;
                    return self.finish_after_handoff_sync(after);
                }
            };
        let omissions = agent.provider_context_omissions(&target_identity);
        let impact = ContextHandoffImpact {
            source_label,
            target_label,
            omissions,
            can_compact: agent.can_compact(),
            offer_use_source: false,
            cache_warm: agent.live_context_warm(),
        };
        if !impact.should_prompt() {
            self.select_model_with_omission_notice(selection, agent)?;
            return self.finish_after_handoff_sync(after);
        }

        let choice = impact.choice(ContextHandoffKind::ModelSwitch)?;
        self.open_context_handoff(
            choice,
            PendingContextHandoff {
                kind: ContextHandoffKind::ModelSwitch,
                source_selection: None,
                target_selection: Some(selection),
                materialize: None,
                after,
            },
        );
        Ok(())
    }

    pub(super) fn offer_resume_context_handoff(
        &mut self,
        session: &Session,
        model_history: &[Message],
        display_history: &[Message],
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let target_identity = agent.provider_identity();
        let omissions =
            rho_sdk::model::handoff::report_message_omissions(model_history, &target_identity);
        if !omissions.has_omissions() {
            return Ok(false);
        }

        let source_identity = session
            .stored_provider_identity()?
            .or_else(|| first_omitted_source_identity(model_history, &target_identity));
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
            can_compact: source_selection.is_some() && agent.can_compact_messages(model_history),
            offer_use_source: source_selection.is_some(),
            cache_warm: false,
        };
        if !impact.should_prompt() {
            return Ok(false);
        }

        let choice = impact.choice(ContextHandoffKind::Resume)?;
        self.open_context_handoff(
            choice,
            PendingContextHandoff {
                kind: ContextHandoffKind::Resume,
                source_selection,
                target_selection: Some(current_runtime_selection(self)),
                materialize: Some(ResumeMaterialize {
                    session: session.clone(),
                    model_history: model_history.to_vec(),
                    display_history: display_history.to_vec(),
                }),
                after: AfterHandoff::None,
            },
        );
        Ok(true)
    }

    pub(super) fn maybe_offer_loaded_session_context_handoff(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if !matches!(self.input_ui.composer, ComposerMode::Input) {
            return Ok(());
        }
        if self.info.session.recovered_messages.is_empty() {
            self.insert_runtime_notices(agent);
            return Ok(());
        }

        // Startup already warned on stderr; drain any structured pending omission
        // without showing a duplicate notice if we are about to prompt.
        let target_identity = agent.provider_identity();
        let omissions = agent.provider_context_omissions(&target_identity);
        if !omissions.has_omissions() {
            self.insert_runtime_notices(agent);
            return Ok(());
        }
        let _ = agent.take_pending_omission();

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
            offer_use_source: source_selection.is_some(),
            cache_warm: false,
        };
        let choice = impact.choice(ContextHandoffKind::LoadedSession)?;
        self.open_context_handoff(
            choice,
            PendingContextHandoff {
                kind: ContextHandoffKind::LoadedSession,
                source_selection,
                target_selection: Some(current_runtime_selection(self)),
                materialize: None,
                after: AfterHandoff::None,
            },
        );
        Ok(())
    }

    fn open_context_handoff(&mut self, choice: InlineChoice, pending: PendingContextHandoff) {
        let status = match pending.kind {
            ContextHandoffKind::ModelSwitch => "choose model handoff",
            ContextHandoffKind::Resume => "choose resume handoff",
            ContextHandoffKind::LoadedSession => "choose loaded-session handoff",
        };
        self.input_ui.composer = ComposerMode::InlineChoice(InlineChoiceModal {
            choice,
            pending: InlineChoicePending::ContextHandoff(Box::new(pending)),
        });
        self.status = status.into();
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

    pub(super) async fn resolve_context_handoff(
        &mut self,
        value: Option<&str>,
        pending: PendingContextHandoff,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let PendingContextHandoff {
            kind,
            source_selection,
            target_selection,
            materialize,
            after,
        } = pending;
        if let Some(value) = value {
            let decision =
                decision_from_value(value).unwrap_or(ContextHandoffDecision::ContinueDirect);
            if let Err(err) = self
                .execute_context_handoff(
                    decision,
                    source_selection,
                    target_selection,
                    materialize,
                    terminal,
                    agent,
                )
                .await
            {
                self.insert_entry(&Entry::Error(format!("model handoff failed: {err}")));
                self.status = "model handoff failed".into();
            }
        } else {
            self.status = match kind {
                ContextHandoffKind::ModelSwitch => "model switch cancelled".into(),
                ContextHandoffKind::Resume => "resume cancelled".into(),
                ContextHandoffKind::LoadedSession => "ready".into(),
            };
        }
        self.finish_after_handoff(after, terminal, agent).await
    }

    async fn execute_context_handoff(
        &mut self,
        decision: ContextHandoffDecision,
        source_selection: Option<InteractiveModelSelection>,
        target_selection: Option<InteractiveModelSelection>,
        materialize: Option<ResumeMaterialize>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match decision {
            ContextHandoffDecision::UseSourceModel => {
                let Some(source) = source_selection else {
                    anyhow::bail!("session model is unavailable");
                };
                self.select_model(source, agent)?;
                self.materialize_if_needed(materialize, terminal, agent)
                    .await?;
            }
            ContextHandoffDecision::CompactThenContinue => {
                let had_source = source_selection.is_some();
                if let Some(source) = source_selection {
                    if !selection_matches_runtime(self, &source) {
                        self.select_model(source, agent)?;
                    }
                }
                self.materialize_if_needed(materialize, terminal, agent)
                    .await?;
                match self.execute_compact_command(terminal, agent).await {
                    Ok(true) => {
                        if let Some(target) = target_selection {
                            if !selection_matches_runtime(self, &target) {
                                self.select_model(target, agent)?;
                            }
                        }
                    }
                    Ok(false) => {
                        let notice = if had_source {
                            "kept the session model because context was not compacted"
                        } else {
                            "model unchanged because context was not compacted"
                        };
                        self.insert_entry(&Entry::Notice(notice.into()));
                    }
                    Err(err) => return Err(err),
                }
            }
            ContextHandoffDecision::ContinueDirect => {
                let materialized = materialize.is_some();
                self.materialize_if_needed(materialize, terminal, agent)
                    .await?;
                if let Some(target) = target_selection {
                    if !selection_matches_runtime(self, &target) {
                        self.select_model(target, agent)?;
                    } else if !materialized {
                        self.status = "ready".into();
                    }
                }
            }
        }
        Ok(())
    }

    async fn materialize_if_needed(
        &mut self,
        materialize: Option<ResumeMaterialize>,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(materialize) = materialize else {
            return Ok(());
        };
        self.apply_resume_session(
            materialize.session,
            materialize.model_history,
            materialize.display_history,
            terminal,
            agent,
        )
        .await
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
        // User already confirmed handoff when this path runs after a prompt; drop
        // the structured omission so we do not re-announce it as a string notice.
        let _ = agent.take_pending_omission();
        self.info.session.session_id = Some(full_id);
        self.info.session.recovered_messages = display_history.clone();
        self.input_ui.composer = ComposerMode::Input;
        self.input_ui.text.clear();
        self.input_ui.paste_segments.clear();
        self.input_ui.shell_mode = None;
        self.input_ui.cursor = 0;
        self.input_ui.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.reset_streams();
        self.end_busy_ui();
        self.goal = None;
        self.reset_usage();
        self.usage.current_context = None;
        let entries = transcript_entries_from_messages(&display_history, &self.info.runtime.cwd);
        let width = terminal.size()?.width as usize;
        let (_omitted, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.runtime.max_tool_output_lines,
        );
        self.history.set_entries(visible_entries);
        self.history.images_mut().clear();
        self.history.invalidate_from(0);
        self.history
            .set_last_inserted_was_tool(self.history.last().is_some_and(is_tool_entry));
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

    async fn finish_after_handoff(
        &mut self,
        after: AfterHandoff,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match after {
            AfterHandoff::None => Ok(()),
            AfterHandoff::ContinueTurnWork => {
                self.continue_after_model_handoff(terminal, agent).await
            }
            AfterHandoff::ReopenConfigPicker { picker, selected } => {
                self.open_main_config_picker(&selected, picker.filter)
            }
        }
    }

    fn finish_after_handoff_sync(&mut self, after: AfterHandoff) -> anyhow::Result<()> {
        match after {
            AfterHandoff::None | AfterHandoff::ContinueTurnWork => Ok(()),
            AfterHandoff::ReopenConfigPicker { picker, selected } => {
                self.open_main_config_picker(&selected, picker.filter)
            }
        }
    }

    async fn continue_after_model_handoff(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if let Some(prompt) = self.pending.queued_prompts.pop_front() {
            self.restore_pending_prompt(prompt);
            return self.submit(terminal, agent).await;
        }
        if self.goal.is_some() && !self.should_quit {
            self.continue_goal(terminal, agent, std::collections::VecDeque::new())
                .await?;
        }
        Ok(())
    }

    pub(super) fn select_model_with_omission_notice(
        &mut self,
        selection: InteractiveModelSelection,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let report = self.select_model_report(selection, agent)?;
        if let Some(report) = report.filter(HandoffReport::has_omissions) {
            self.insert_entry(&Entry::Notice(format!(
                "model handoff omitted {} nonportable provider context block(s): {}; assistant text, tool history, and reasoning summaries were preserved",
                report.omitted_provider_context,
                report.omitted_kinds.join(", ")
            )));
        }
        Ok(())
    }
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

fn selection_matches_runtime(app: &App, selection: &InteractiveModelSelection) -> bool {
    selection.selection.provider == app.info.runtime.provider
        && selection.selection.model == app.info.runtime.model
        && selection.selection.auth == app.info.runtime.auth
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

fn next_shortcut(current: char) -> char {
    char::from_u32(u32::from(current) + 1).unwrap_or(current)
}

#[cfg(test)]
#[path = "context_handoff_tests.rs"]
mod tests;
