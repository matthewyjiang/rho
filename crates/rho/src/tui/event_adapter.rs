use rho_sdk::{
    model::{ContextUsage, ModelUsage},
    HostInputRequest, HostInputResponse, ModelCallMetrics, ModelCallProfile, RunEvent,
};
use {
    crate::app::interactive_presenter::InteractiveToolPresenter,
    crate::questionnaire::{QuestionnaireAnswer, QuestionnaireQuestionKind, QuestionnaireResponse},
    rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus},
};

use super::{
    activity::ActivityPhase,
    compaction_display::compaction_call_id,
    questionnaire::{QuestionnaireChoice, QuestionnaireQuestion, QuestionnaireRequest},
};

pub(super) use super::compaction_display::{CompactionDisplayFacts, CompactionUiOutcome};

#[derive(Clone, Debug)]
pub(super) enum ViewModelEvent {
    RunStarted,
    StepStarted(usize),
    SteeringApplied(Vec<rho_sdk::SteeringId>),
    ToolStarted {
        call_id: rho_sdk::ToolCallId,
        card: ToolCard,
    },
    ProviderStreamReset,
    ProviderRetry,
    OutputDelta(String),
    ReasoningDelta(String),
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    ModelCallCompleted {
        profile: ModelCallProfile,
        metrics: ModelCallMetrics,
    },
    ToolUpdated {
        call_id: rho_sdk::ToolCallId,
        card: ToolCard,
    },
    ToolCallUpdated {
        index: usize,
        call_id: Option<rho_sdk::ToolCallId>,
        /// Present when the streamed preview card changed. Identity-only binds
        /// may omit a card so the batch can attach a late call id without a
        /// forced re-render.
        card: Option<ToolCard>,
    },
    /// Final proposal for a tool call, keyed only by call id.
    ///
    /// Distinct from stream `ToolCallUpdated` so proposal never invents a dense
    /// index in the provider output_index namespace.
    ToolCallProposed {
        call_id: rho_sdk::ToolCallId,
        card: ToolCard,
    },
    ToolFinished {
        call_id: rho_sdk::ToolCallId,
        card: ToolCard,
        image_asset: Option<rho_sdk::tool::ToolAsset>,
    },
}

impl ViewModelEvent {
    pub(super) fn activity_phase(&self) -> Option<ActivityPhase> {
        match self {
            Self::RunStarted => Some(ActivityPhase::Starting),
            // Compaction is tool-shaped; keep the dedicated activity phase via call id.
            Self::ToolFinished { call_id, .. } if call_id == &compaction_call_id() => {
                Some(ActivityPhase::Starting)
            }
            Self::ToolFinished { .. } => None,
            Self::StepStarted(_) => Some(ActivityPhase::WaitingForProvider),
            Self::ToolStarted { call_id, .. } if call_id == &compaction_call_id() => {
                Some(ActivityPhase::Compacting)
            }
            Self::ToolStarted { .. } | Self::ToolUpdated { .. } => Some(ActivityPhase::RunningTool),
            Self::ToolCallUpdated { .. } | Self::ToolCallProposed { .. } => {
                Some(ActivityPhase::PreparingTool)
            }
            Self::ProviderStreamReset | Self::ProviderRetry => {
                Some(ActivityPhase::RetryingProvider)
            }
            Self::OutputDelta(_) => Some(ActivityPhase::Responding),
            Self::ReasoningDelta(_) => Some(ActivityPhase::Thinking),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum ViewEvent {
    Update(ViewModelEvent),
    Questionnaire {
        call_id: rho_sdk::ToolCallId,
        request: HostInputRequest,
    },
    Notice(String),
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Default)]
pub(crate) struct SdkEventAdapter {
    presenter: Option<InteractiveToolPresenter>,
    compaction_open: bool,
    /// Provider output_index -> call id already emitted for a streamed preview.
    bound_stream_call_ids: std::collections::BTreeMap<usize, String>,
    /// Provider output_index -> attachment journal key for live tool previews.
    attachment_preview_keys: std::collections::BTreeMap<usize, String>,
    /// call_id -> attachment journal key so later events reuse the preview slot.
    attachment_call_keys: std::collections::BTreeMap<String, String>,
}

impl SdkEventAdapter {
    pub(crate) fn new(cwd: std::path::PathBuf) -> Self {
        Self {
            presenter: Some(InteractiveToolPresenter::new(cwd)),
            compaction_open: false,
            bound_stream_call_ids: std::collections::BTreeMap::new(),
            attachment_preview_keys: std::collections::BTreeMap::new(),
            attachment_call_keys: std::collections::BTreeMap::new(),
        }
    }

    fn presenter(&mut self) -> &mut InteractiveToolPresenter {
        self.presenter
            .get_or_insert_with(|| InteractiveToolPresenter::new(std::path::PathBuf::new()))
    }

    fn close_compaction(&mut self, outcome: CompactionUiOutcome) -> Option<ViewEvent> {
        if !self.compaction_open {
            return None;
        }
        self.compaction_open = false;
        Some(ViewEvent::Update(compaction_finished(outcome)))
    }

    /// Stable attachment key for a streaming preview, reusing the index slot when
    /// a call id arrives later.
    pub(crate) fn attachment_key_for_preview(
        &mut self,
        index: usize,
        call_id: Option<&rho_sdk::ToolCallId>,
    ) -> String {
        if let Some(call_id) = call_id {
            let id = call_id.to_string();
            if let Some(key) = self.attachment_call_keys.get(&id).cloned() {
                self.attachment_preview_keys.insert(index, key.clone());
                return key;
            }
            if let Some(key) = self.attachment_preview_keys.get(&index).cloned() {
                self.attachment_call_keys.insert(id, key.clone());
                return key;
            }
            self.attachment_preview_keys.insert(index, id.clone());
            self.attachment_call_keys.insert(id.clone(), id.clone());
            return id;
        }
        self.attachment_preview_keys
            .entry(index)
            .or_insert_with(|| format!("preview:{index}"))
            .clone()
    }

    /// Attachment key for a call-id-addressed event, preferring any prior preview.
    pub(crate) fn attachment_key_for_call(&mut self, call_id: &rho_sdk::ToolCallId) -> String {
        let id = call_id.to_string();
        self.attachment_call_keys
            .entry(id.clone())
            .or_insert(id)
            .clone()
    }

    /// Consume the key for a finished call so a later call can reuse the slot.
    pub(crate) fn take_attachment_key_for_call(&mut self, call_id: &rho_sdk::ToolCallId) -> String {
        let id = call_id.to_string();
        let key = self
            .attachment_call_keys
            .remove(&id)
            .unwrap_or_else(|| id.clone());
        self.attachment_preview_keys
            .retain(|_, existing| existing != &key);
        key
    }

    pub(crate) fn clear_attachment_preview_keys(&mut self) {
        self.bound_stream_call_ids.clear();
        self.attachment_preview_keys.clear();
        self.attachment_call_keys.clear();
    }

    /// Translates one SDK run event into zero or more view events.
    ///
    /// Compaction failures and cancellations emit a finished tool block before
    /// the run terminal event so auto-compact shares the manual lifecycle.
    pub(super) fn translate(&mut self, event: RunEvent) -> Vec<ViewEvent> {
        match event {
            RunEvent::Started { .. } => {
                vec![ViewEvent::Update(ViewModelEvent::RunStarted)]
            }
            RunEvent::StepStarted { step } => {
                self.presenter().step_started();
                self.bound_stream_call_ids.clear();
                vec![ViewEvent::Update(ViewModelEvent::StepStarted(step))]
            }
            RunEvent::SteeringApplied { ids } => {
                vec![ViewEvent::Update(ViewModelEvent::SteeringApplied(ids))]
            }
            RunEvent::AssistantTextDelta { text } => {
                vec![ViewEvent::Update(ViewModelEvent::OutputDelta(text))]
            }
            RunEvent::ReasoningDelta { text } | RunEvent::ReasoningSummaryDelta { text } => {
                vec![ViewEvent::Update(ViewModelEvent::ReasoningDelta(text))]
            }
            RunEvent::ToolCallUpdated {
                index,
                id,
                name,
                arguments_delta,
            } => {
                // StreamCapture re-emits known identity on later deltas, so the
                // first rendered preview can bind the call-id slot.
                let call_id = id.and_then(|id| rho_sdk::ToolCallId::from_string(id).ok());
                let newly_bound = call_id.as_ref().is_some_and(|call_id| {
                    self.bound_stream_call_ids
                        .insert(index, call_id.to_string())
                        .as_deref()
                        != Some(call_id.as_str())
                });
                let card = self
                    .presenter()
                    .preview(index, name, &arguments_delta)
                    .map(|presented| presented.card);
                // Emit when the card changed, or when a late call id needs to
                // bind an existing preview slot without forcing a re-render.
                if card.is_none() && !newly_bound {
                    Vec::new()
                } else {
                    vec![ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
                        index,
                        call_id,
                        card,
                    })]
                }
            }
            RunEvent::ToolProposed { call } => {
                let Ok(call_id) = rho_sdk::ToolCallId::from_string(call.id.clone()) else {
                    return Vec::new();
                };
                let presented = self.presenter().proposed(call);
                vec![ViewEvent::Update(ViewModelEvent::ToolCallProposed {
                    call_id,
                    card: presented.card,
                })]
            }
            RunEvent::ToolStarted {
                call_id,
                name,
                metadata,
            } => {
                let presented = self.presenter().started(call_id.clone(), name, metadata);
                vec![ViewEvent::Update(ViewModelEvent::ToolStarted {
                    call_id,
                    card: presented.card,
                })]
            }
            RunEvent::ToolUpdated { call_id, progress } => {
                let presented = self.presenter().updated(&call_id, &progress);
                vec![ViewEvent::Update(ViewModelEvent::ToolUpdated {
                    call_id,
                    card: presented.card,
                })]
            }
            RunEvent::ToolFinished { call_id, result } => {
                let (_ok, presented) = self.presenter().finished(&call_id, result);
                vec![ViewEvent::Update(ViewModelEvent::ToolFinished {
                    call_id,
                    card: presented.card,
                    image_asset: presented.image_asset,
                })]
            }
            RunEvent::UsageUpdated { usage } => {
                vec![ViewEvent::Update(ViewModelEvent::Usage(usage))]
            }
            RunEvent::ModelCallCompleted { profile, metrics } => {
                vec![ViewEvent::Update(ViewModelEvent::ModelCallCompleted {
                    profile,
                    metrics,
                })]
            }
            RunEvent::WebSearch { detail } => {
                vec![provider_native_activity_finished(
                    "web_search",
                    detail,
                    ToolFamily::Web,
                )]
            }
            RunEvent::HostedToolActivity { name, detail } => {
                vec![provider_native_activity_finished(
                    &name,
                    detail,
                    hosted_tool_family(&name),
                )]
            }
            RunEvent::ProviderRequestRetry => {
                vec![ViewEvent::Update(ViewModelEvent::ProviderRetry)]
            }
            // Legacy dual-emitted activity; typed variants above drive the TUI.
            #[allow(deprecated)]
            RunEvent::ProviderActivity { .. } => Vec::new(),
            RunEvent::ProviderStreamReset { .. } => {
                self.presenter().step_started();
                self.bound_stream_call_ids.clear();
                vec![ViewEvent::Update(ViewModelEvent::ProviderStreamReset)]
            }
            RunEvent::ProviderContextUpdated { .. } => Vec::new(),
            RunEvent::ProviderDiagnostic { detail } => {
                vec![ViewEvent::Notice(format!(
                    "provider diagnostic:\n{}",
                    detail.as_str()
                ))]
            }
            RunEvent::HostInputRequested { request } => {
                vec![ViewEvent::Questionnaire {
                    call_id: rho_sdk::ToolCallId::new(),
                    request,
                }]
            }
            RunEvent::ToolHostInputRequested { call_id, request } => {
                vec![ViewEvent::Questionnaire { call_id, request }]
            }
            RunEvent::CompactionStarted { .. } => {
                self.compaction_open = true;
                vec![ViewEvent::Update(compaction_started())]
            }
            RunEvent::CompactionCompleted { outcome, .. } => {
                self.compaction_open = false;
                vec![ViewEvent::Update(compaction_finished(
                    CompactionUiOutcome::Completed(CompactionDisplayFacts::from_outcome(&outcome)),
                ))]
            }
            RunEvent::Completed { .. } => {
                let mut events = Vec::new();
                if let Some(finished) = self.close_compaction(CompactionUiOutcome::Unchanged {
                    detail: "compaction ended without a result".into(),
                }) {
                    events.push(finished);
                }
                events.push(ViewEvent::Completed);
                events
            }
            RunEvent::Cancelled { .. } => {
                let mut events = Vec::new();
                if let Some(finished) = self.close_compaction(CompactionUiOutcome::Cancelled) {
                    events.push(finished);
                }
                events.push(ViewEvent::Cancelled);
                events
            }
            RunEvent::Failed { message, .. } => {
                let mut events = Vec::new();
                if let Some(finished) = self.close_compaction(CompactionUiOutcome::Failed {
                    detail: message.clone(),
                }) {
                    events.push(finished);
                }
                events.push(ViewEvent::Failed(message));
                events
            }
            _ => Vec::new(),
        }
    }
}

fn compaction_started() -> ViewModelEvent {
    ViewModelEvent::ToolStarted {
        call_id: compaction_call_id(),
        card: super::compaction_display::running_card(),
    }
}

fn hosted_tool_family(name: &str) -> ToolFamily {
    match name {
        // Hosted X Search is a network lookup; keep the web chrome.
        "x_search" => ToolFamily::Web,
        _ => ToolFamily::Default,
    }
}

fn provider_native_activity_finished(name: &str, detail: String, family: ToolFamily) -> ViewEvent {
    ViewEvent::Update(ViewModelEvent::ToolFinished {
        call_id: rho_sdk::ToolCallId::new(),
        card: ToolCard::new(ToolStatus::Ok, family, ToolHeader::call(name, Some(detail))),
        image_asset: None,
    })
}

fn compaction_finished(outcome: CompactionUiOutcome) -> ViewModelEvent {
    ViewModelEvent::ToolFinished {
        call_id: compaction_call_id(),
        card: outcome.card(),
        image_asset: None,
    }
}

pub(super) fn questionnaire_request(request: &HostInputRequest) -> QuestionnaireRequest {
    QuestionnaireRequest {
        title: (!request.title().is_empty()).then(|| request.title().to_string()),
        reason: None,
        questions: request
            .questions()
            .iter()
            .map(|question| {
                let choices = question
                    .choices()
                    .iter()
                    .map(|choice| {
                        let mapped = QuestionnaireChoice::new(choice.value(), choice.label());
                        match choice.description_text() {
                            Some(description) => mapped.description(description),
                            None => mapped,
                        }
                    })
                    .collect::<Vec<_>>();
                QuestionnaireQuestion {
                    id: question.id().to_string(),
                    question: question.prompt().to_string(),
                    header: question.header_text().map(str::to_string),
                    help: question.help_text().map(str::to_string),
                    default: question.default_value_ref().cloned(),
                    default_selection: question.default_selection_mode().into(),
                    kind: questionnaire_kind(question),
                    required: question.is_required(),
                    choices,
                    allow_other: question.permits_other(),
                }
            })
            .collect(),
    }
}

fn questionnaire_kind(question: &rho_sdk::HostQuestion) -> QuestionnaireQuestionKind {
    match question.selection() {
        rho_sdk::SelectionMode::Many => QuestionnaireQuestionKind::MultiSelect,
        rho_sdk::SelectionMode::One if is_yes_no_question(question) => {
            QuestionnaireQuestionKind::Confirm
        }
        rho_sdk::SelectionMode::One => QuestionnaireQuestionKind::Choice,
        _ => QuestionnaireQuestionKind::Choice,
    }
}

fn is_yes_no_question(question: &rho_sdk::HostQuestion) -> bool {
    matches!(
        question.choices(),
        [yes, no]
            if yes.value().eq_ignore_ascii_case("yes")
                && no.value().eq_ignore_ascii_case("no")
    )
}

pub(super) fn host_response(response: QuestionnaireResponse) -> HostInputResponse {
    response.answers.into_iter().fold(
        HostInputResponse::new(),
        |response, QuestionnaireAnswer { id, answer }| match answer {
            serde_json::Value::Null => response,
            serde_json::Value::Array(values) if values.is_empty() => response,
            serde_json::Value::Array(values) => {
                response.answer(id, values.into_iter().map(answer_text).collect::<Vec<_>>())
            }
            value => response.answer(id, vec![answer_text(value)]),
        },
    )
}

fn answer_text(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value,
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        value => value.to_string(),
    }
}

#[cfg(test)]
#[path = "event_adapter_tests.rs"]
mod tests;
