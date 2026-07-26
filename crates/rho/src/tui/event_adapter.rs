use rho_sdk::{
    model::{ContextUsage, ModelUsage},
    HostInputRequest, HostInputResponse, RunEvent,
};
use {
    crate::app::interactive_presenter::InteractiveToolPresenter,
    crate::questionnaire::{QuestionnaireAnswer, QuestionnaireQuestionKind, QuestionnaireResponse},
    rho_tools::tool::ToolDisplayStyle,
};

use super::{
    activity::ActivityPhase,
    compaction_display::{compaction_call_id, running_display_lines},
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
        display_lines: Vec<String>,
    },
    ProviderStreamReset,
    ProviderRetry,
    OutputDelta(String),
    ReasoningDelta(String),
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    ToolUpdated {
        call_id: rho_sdk::ToolCallId,
        display_lines: Vec<String>,
    },
    ToolCallUpdated {
        index: usize,
        call_id: Option<rho_sdk::ToolCallId>,
        display_lines: Vec<String>,
    },
    /// Final proposal for a tool call, keyed only by call id.
    ///
    /// Distinct from stream `ToolCallUpdated` so proposal never invents a dense
    /// index in the provider output_index namespace.
    ToolCallProposed {
        call_id: rho_sdk::ToolCallId,
        display_lines: Vec<String>,
    },
    ToolFinished {
        call_id: rho_sdk::ToolCallId,
        ok: bool,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
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
}

impl SdkEventAdapter {
    pub(crate) fn new(cwd: std::path::PathBuf) -> Self {
        Self {
            presenter: Some(InteractiveToolPresenter::new(cwd)),
            compaction_open: false,
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
                let call_id = id.and_then(|id| rho_sdk::ToolCallId::from_string(id).ok());
                self.presenter()
                    .preview(index, name, &arguments_delta)
                    .map_or_else(Vec::new, |display_lines| {
                        vec![ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
                            index,
                            call_id,
                            display_lines,
                        })]
                    })
            }
            RunEvent::ToolProposed { call } => {
                let Ok(call_id) = rho_sdk::ToolCallId::from_string(call.id.clone()) else {
                    return Vec::new();
                };
                let display_lines = self.presenter().proposed(call);
                vec![ViewEvent::Update(ViewModelEvent::ToolCallProposed {
                    call_id,
                    display_lines,
                })]
            }
            RunEvent::ToolStarted {
                call_id,
                name,
                metadata,
            } => {
                let display_lines = self
                    .presenter()
                    .started(call_id.clone(), name, metadata)
                    .display_lines;
                vec![ViewEvent::Update(ViewModelEvent::ToolStarted {
                    call_id,
                    display_lines,
                })]
            }
            RunEvent::ToolUpdated { call_id, progress } => {
                let display_lines = self.presenter().updated(&call_id, &progress);
                vec![ViewEvent::Update(ViewModelEvent::ToolUpdated {
                    call_id,
                    display_lines,
                })]
            }
            RunEvent::ToolFinished { call_id, result } => {
                let (ok, presented) = self.presenter().finished(&call_id, result);
                vec![ViewEvent::Update(ViewModelEvent::ToolFinished {
                    call_id,
                    ok,
                    display_style: presented.display_style,
                    display_lines: presented.display_lines,
                    image_asset: presented.image_asset,
                })]
            }
            RunEvent::UsageUpdated { usage } => {
                vec![ViewEvent::Update(ViewModelEvent::Usage(usage))]
            }
            RunEvent::WebSearch { detail } => {
                vec![ViewEvent::Update(ViewModelEvent::ToolFinished {
                    call_id: rho_sdk::ToolCallId::new(),
                    ok: true,
                    display_style: ToolDisplayStyle::web(),
                    display_lines: vec![format!("web search: {detail}")],
                    image_asset: None,
                })]
            }
            RunEvent::ProviderRequestRetry => {
                vec![ViewEvent::Update(ViewModelEvent::ProviderRetry)]
            }
            RunEvent::ProviderStreamReset { .. } => {
                self.presenter().step_started();
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
        display_lines: running_display_lines(),
    }
}

fn compaction_finished(outcome: CompactionUiOutcome) -> ViewModelEvent {
    ViewModelEvent::ToolFinished {
        call_id: compaction_call_id(),
        ok: outcome.ok(),
        display_style: ToolDisplayStyle::default_tool(),
        display_lines: outcome.display_lines(),
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
