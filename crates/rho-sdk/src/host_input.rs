use std::{collections::BTreeMap, fmt};

use tokio::sync::{mpsc, oneshot};

use crate::{CancellationToken, Error, HostInputId};

use serde_json::Value;

/// One selectable answer in a host question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostChoice {
    value: String,
    label: String,
    description: Option<String>,
}

impl HostChoice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: None,
        }
    }

    /// Add short supporting text that hosts may show below the choice label.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// Selection mode for a host question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionMode {
    One,
    Many,
}

/// One structured question presented by a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostQuestion {
    id: String,
    prompt: String,
    header: Option<String>,
    choices: Vec<HostChoice>,
    selection: SelectionMode,
    allow_other: bool,
    help: Option<String>,
    default: Option<Value>,
    required: bool,
}

impl HostQuestion {
    pub fn new(
        id: impl Into<String>,
        prompt: impl Into<String>,
        choices: Vec<HostChoice>,
        selection: SelectionMode,
    ) -> Result<Self, Error> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::InvalidHostResponse {
                message: "host question ID must not be empty".into(),
            });
        }
        if choices.is_empty() {
            return Err(Error::InvalidHostResponse {
                message: format!("host question '{id}' must have at least one choice"),
            });
        }
        let mut values = std::collections::BTreeSet::new();
        if choices.iter().any(|choice| !values.insert(&choice.value)) {
            return Err(Error::InvalidHostResponse {
                message: format!("host question '{id}' choice values must be unique"),
            });
        }
        Ok(Self {
            id,
            prompt: prompt.into(),
            header: None,
            choices,
            selection,
            allow_other: false,
            help: None,
            default: None,
            required: true,
        })
    }

    pub fn allow_other(mut self) -> Self {
        self.allow_other = true;
        self
    }

    /// Very short label hosts may show in place of the full prompt where
    /// space is tight, such as a question tab.
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn default_value(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn header_text(&self) -> Option<&str> {
        self.header.as_deref()
    }

    pub fn choices(&self) -> &[HostChoice] {
        &self.choices
    }

    pub fn selection(&self) -> SelectionMode {
        self.selection
    }

    pub fn permits_other(&self) -> bool {
        self.allow_other
    }

    pub fn help_text(&self) -> Option<&str> {
        self.help.as_deref()
    }

    pub fn default_value_ref(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// Typed questionnaire request emitted by an SDK run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInputRequest {
    id: HostInputId,
    title: String,
    questions: Vec<HostQuestion>,
}

impl HostInputRequest {
    pub fn questionnaire(
        title: impl Into<String>,
        questions: Vec<HostQuestion>,
    ) -> Result<Self, Error> {
        if questions.is_empty() {
            return Err(Error::InvalidHostResponse {
                message: "host questionnaire must contain at least one question".into(),
            });
        }
        let mut ids = std::collections::BTreeSet::new();
        if questions.iter().any(|question| !ids.insert(&question.id)) {
            return Err(Error::InvalidHostResponse {
                message: "host questionnaire question IDs must be unique".into(),
            });
        }
        Ok(Self {
            id: HostInputId::new(),
            title: title.into(),
            questions,
        })
    }

    pub fn id(&self) -> &HostInputId {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn questions(&self) -> &[HostQuestion] {
        &self.questions
    }

    pub fn validate(&self, response: &HostInputResponse) -> Result<(), Error> {
        if let Some(question_id) = response
            .answers
            .keys()
            .find(|id| !self.questions.iter().any(|question| &question.id == *id))
        {
            return Err(Error::InvalidHostResponse {
                message: format!("host response contains unknown question '{question_id}'"),
            });
        }
        for question in &self.questions {
            let Some(answers) = response.answers.get(&question.id) else {
                if question.required {
                    return Err(Error::InvalidHostResponse {
                        message: format!("host response is missing question '{}'", question.id),
                    });
                }
                continue;
            };
            if (answers.is_empty() && question.required)
                || (question.selection == SelectionMode::One && answers.len() > 1)
            {
                return Err(Error::InvalidHostResponse {
                    message: format!("invalid answer count for question '{}'", question.id),
                });
            }
            let mut unique = std::collections::BTreeSet::new();
            for answer in answers {
                if !unique.insert(answer) {
                    return Err(Error::InvalidHostResponse {
                        message: format!("duplicate answer for question '{}'", question.id),
                    });
                }
                let known = question
                    .choices
                    .iter()
                    .any(|choice| choice.value == *answer);
                if !known && !question.allow_other {
                    return Err(Error::InvalidHostResponse {
                        message: format!("unknown answer for question '{}'", question.id),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Structured host answers keyed by question ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInputResponse {
    answers: BTreeMap<String, Vec<String>>,
}

impl HostInputResponse {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn answer(
        mut self,
        question_id: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.answers.insert(
            question_id.into(),
            values.into_iter().map(Into::into).collect(),
        );
        self
    }

    pub fn answers(&self) -> &BTreeMap<String, Vec<String>> {
        &self.answers
    }
}

pub(crate) struct HostInputEnvelope {
    pub(crate) request: HostInputRequest,
    pub(crate) response: oneshot::Sender<Result<HostInputResponse, Error>>,
}

#[derive(Clone)]
pub(crate) struct HostInputRequester {
    sender: mpsc::Sender<HostInputEnvelope>,
    cancellation: CancellationToken,
}

impl HostInputRequester {
    pub(crate) fn new(
        sender: mpsc::Sender<HostInputEnvelope>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            sender,
            cancellation,
        }
    }

    pub(crate) async fn request(
        &self,
        request: HostInputRequest,
    ) -> Result<HostInputResponse, Error> {
        let (response, receiver) = oneshot::channel();
        tokio::select! {
            result = self.sender.send(HostInputEnvelope { request, response }) => {
                result.map_err(|_| Error::Interrupted {
                    message: "run stopped accepting host input requests".into(),
                })?;
            }
            () = self.cancellation.cancelled() => return Err(Error::Cancelled),
        }
        tokio::select! {
            result = receiver => result.map_err(|_| Error::Interrupted {
                message: "host input request was dropped without a response".into(),
            })?,
            () = self.cancellation.cancelled() => Err(Error::Cancelled),
        }
    }
}

impl fmt::Debug for HostInputRequester {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostInputRequester")
            .field("cancelled", &self.cancellation.is_cancelled())
            .finish_non_exhaustive()
    }
}

pub(crate) fn channel(
    capacity: usize,
    cancellation: CancellationToken,
) -> (HostInputRequester, mpsc::Receiver<HostInputEnvelope>) {
    let (sender, receiver) = mpsc::channel(capacity);
    (HostInputRequester::new(sender, cancellation), receiver)
}

#[cfg(test)]
#[path = "host_input_tests.rs"]
mod tests;
