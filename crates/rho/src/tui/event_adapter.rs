use rho_sdk::{
    model::{ContextUsage, ModelUsage},
    HostInputRequest, HostInputResponse, RunEvent, PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY,
    PROVIDER_ACTIVITY_REQUEST_RETRY, PROVIDER_ACTIVITY_WEB_SEARCH,
};
use {
    crate::app::interactive_presenter::InteractiveToolPresenter,
    crate::questionnaire::{QuestionnaireAnswer, QuestionnaireQuestionKind, QuestionnaireResponse},
    rho_tools::tool::ToolDisplayStyle,
};

use super::{
    activity::ActivityPhase,
    questionnaire::{QuestionnaireChoice, QuestionnaireQuestion, QuestionnaireRequest},
};

pub(super) const COMPACTION_STARTED_NOTICE: &str = "compacting conversation context";

pub(super) fn compaction_completed_notice(
    previous_messages: usize,
    current_messages: usize,
) -> String {
    format!("compacted conversation context ({previous_messages} to {current_messages} messages)")
}

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
    CompactionStarted,
    CompactionCompleted {
        previous_messages: usize,
        current_messages: usize,
    },
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
            Self::RunStarted | Self::CompactionCompleted { .. } => Some(ActivityPhase::Starting),
            Self::ToolFinished { .. } => None,
            Self::StepStarted(_) => Some(ActivityPhase::WaitingForProvider),
            Self::ToolStarted { .. } | Self::ToolUpdated { .. } => Some(ActivityPhase::RunningTool),
            Self::ToolCallUpdated { .. } => Some(ActivityPhase::PreparingTool),
            Self::ProviderStreamReset | Self::ProviderRetry => {
                Some(ActivityPhase::RetryingProvider)
            }
            Self::OutputDelta(_) => Some(ActivityPhase::Responding),
            Self::ReasoningDelta(_) => Some(ActivityPhase::Thinking),
            Self::CompactionStarted => Some(ActivityPhase::Compacting),
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
    Ignored,
}

#[derive(Default)]
pub(super) struct SdkEventAdapter {
    presenter: Option<InteractiveToolPresenter>,
    proposed_index: usize,
}

impl SdkEventAdapter {
    pub(super) fn new(cwd: std::path::PathBuf) -> Self {
        Self {
            presenter: Some(InteractiveToolPresenter::new(cwd)),
            proposed_index: 0,
        }
    }

    fn presenter(&mut self) -> &mut InteractiveToolPresenter {
        self.presenter
            .get_or_insert_with(|| InteractiveToolPresenter::new(std::path::PathBuf::new()))
    }

    pub(super) fn translate(&mut self, event: RunEvent) -> ViewEvent {
        match event {
            RunEvent::Started { .. } => ViewEvent::Update(ViewModelEvent::RunStarted),
            RunEvent::StepStarted { step } => {
                self.presenter().step_started();
                self.proposed_index = 0;
                ViewEvent::Update(ViewModelEvent::StepStarted(step))
            }
            RunEvent::SteeringApplied { ids } => {
                ViewEvent::Update(ViewModelEvent::SteeringApplied(ids))
            }
            RunEvent::AssistantTextDelta { text } => {
                ViewEvent::Update(ViewModelEvent::OutputDelta(text))
            }
            RunEvent::ReasoningDelta { text } | RunEvent::ReasoningSummaryDelta { text } => {
                ViewEvent::Update(ViewModelEvent::ReasoningDelta(text))
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
                    .map_or(ViewEvent::Ignored, |display_lines| {
                        ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
                            index,
                            call_id,
                            display_lines,
                        })
                    })
            }
            RunEvent::ToolProposed { call } => {
                let call_id = rho_sdk::ToolCallId::from_string(call.id.clone()).ok();
                let display_lines = self.presenter().proposed(call);
                let index = self.proposed_index;
                self.proposed_index += 1;
                ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
                    index,
                    call_id,
                    display_lines,
                })
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
                ViewEvent::Update(ViewModelEvent::ToolStarted {
                    call_id,
                    display_lines,
                })
            }
            RunEvent::ToolUpdated { call_id, progress } => {
                let display_lines = self.presenter().updated(&call_id, &progress);
                ViewEvent::Update(ViewModelEvent::ToolUpdated {
                    call_id,
                    display_lines,
                })
            }
            RunEvent::ToolFinished { call_id, result } => {
                let (ok, presented) = self.presenter().finished(&call_id, result);
                ViewEvent::Update(ViewModelEvent::ToolFinished {
                    call_id,
                    ok,
                    display_style: presented.display_style,
                    display_lines: presented.display_lines,
                    image_asset: presented.image_asset,
                })
            }
            RunEvent::UsageUpdated { usage } => ViewEvent::Update(ViewModelEvent::Usage(usage)),
            RunEvent::ProviderActivity { kind, detail } => {
                if kind == PROVIDER_ACTIVITY_WEB_SEARCH {
                    ViewEvent::Update(ViewModelEvent::ToolFinished {
                        call_id: rho_sdk::ToolCallId::new(),
                        ok: true,
                        display_style: ToolDisplayStyle::web(),
                        display_lines: vec![format!("web search: {detail}")],
                        image_asset: None,
                    })
                } else if kind == PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY {
                    // The following typed reset event drives current hosts.
                    ViewEvent::Ignored
                } else if kind == PROVIDER_ACTIVITY_REQUEST_RETRY {
                    ViewEvent::Update(ViewModelEvent::ProviderRetry)
                } else {
                    ViewEvent::Notice(format!("{kind}: {detail}"))
                }
            }
            RunEvent::ProviderStreamReset { .. } => {
                self.presenter().step_started();
                ViewEvent::Update(ViewModelEvent::ProviderStreamReset)
            }
            RunEvent::ProviderContextUpdated { .. } => ViewEvent::Ignored,
            RunEvent::ProviderDiagnostic { detail } => {
                ViewEvent::Notice(format!("provider diagnostic:\n{}", detail.as_str()))
            }
            RunEvent::HostInputRequested { request } => ViewEvent::Questionnaire {
                call_id: rho_sdk::ToolCallId::new(),
                request,
            },
            RunEvent::ToolHostInputRequested { call_id, request } => {
                ViewEvent::Questionnaire { call_id, request }
            }
            RunEvent::CompactionStarted { .. } => {
                ViewEvent::Update(ViewModelEvent::CompactionStarted)
            }
            RunEvent::CompactionCompleted { outcome, .. } => {
                ViewEvent::Update(ViewModelEvent::CompactionCompleted {
                    previous_messages: outcome.previous_messages(),
                    current_messages: outcome.current_messages(),
                })
            }
            RunEvent::Completed { .. } => ViewEvent::Completed,
            RunEvent::Cancelled { .. } => ViewEvent::Cancelled,
            RunEvent::Failed { message, .. } => ViewEvent::Failed(message),
            _ => ViewEvent::Ignored,
        }
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
