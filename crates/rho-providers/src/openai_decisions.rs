//! A [`DecisionModel`] served over OpenAI's Decisions API
//! (`POST {api_base}/decisions`), such as `gpt-6-luna`.
//!
//! The SDK's question kinds map one to one: noul to `predicate`, choice to
//! `choice`, score to `score`. The API has no field for a request's shared
//! instructions, so each question's instructions start with them, and no
//! field for what a predicate's yes and no mean, so its instructions end with
//! them. Errors name only the HTTP status and the request's own names, never
//! text from the response, which a server or proxy may fill with the
//! request's credentials.

use std::{collections::HashMap, hash::Hash, time::Duration};

use rho_sdk::{
    decision::{
        Answer, ChoiceAnswer, DecisionError, DecisionFuture, DecisionModel, DecisionRequest,
        NoulAnswer, Question, QuestionKind, ScoreAnswer,
    },
    model::context::estimate_text_tokens,
    CancellationToken, SecretString,
};
use serde::{Deserialize, Serialize};
use url::Url;

/// OpenAI's public API base, where `/decisions` is served.
pub const OPENAI_API_BASE: &str = "https://api.openai.com/v1";

/// Tripwire for one decision. Not measured on OpenAI (the API documents
/// decisions as about 10x faster than Responses); this is the System One
/// client's 30 s, measured on a cold local model load, so a hosted call that
/// hits it is stuck, not slow.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Largest state, in
/// [`estimate_text_tokens`](rho_sdk::model::context::estimate_text_tokens).
/// `gpt-6-luna`, the only decision model OpenAI serves, takes at most 922,000
/// input tokens (its model page). The estimate assumes 0.25 tokens per char
/// where code has measured up to 0.37 on decision-model tokenizers, so with
/// 1,024 tokens for instructions and questions the budget is
/// (922,000 - 1,024) / 1.5 = 613,984, rounded down.
const STATE_BUDGET: u64 = 613_000;

/// A decision model on OpenAI's Decisions API.
pub struct OpenAiDecisionsModel {
    client: reqwest::Client,
    url: Url,
    model: String,
    api_key: Option<SecretString>,
}

/// Why an [`OpenAiDecisionsModel`] could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenAiDecisionsSetupError {
    #[error("the OpenAI base URL cannot take a path")]
    BaseUrl,
    #[error("failed to build the OpenAI decisions HTTP client")]
    Client(#[source] reqwest::Error),
}

impl OpenAiDecisionsModel {
    /// `api_base` is the API base, such as [`OPENAI_API_BASE`]; requests go
    /// to `{api_base}/decisions`. An `api_key` is sent as a bearer token.
    pub fn new(
        api_base: &Url,
        model: impl Into<String>,
        api_key: Option<SecretString>,
    ) -> Result<Self, OpenAiDecisionsSetupError> {
        let mut url = api_base.clone();
        url.path_segments_mut()
            .map_err(|()| OpenAiDecisionsSetupError::BaseUrl)?
            .pop_if_empty()
            .push("decisions");
        let client =
            crate::decision_http::client(TIMEOUT).map_err(OpenAiDecisionsSetupError::Client)?;
        Ok(Self {
            client,
            url,
            model: model.into(),
            api_key,
        })
    }

    /// The model name sent with each request.
    pub fn model(&self) -> &str {
        &self.model
    }

    async fn decide_now(
        &self,
        request: DecisionRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Answer>, DecisionError> {
        request.check().map_err(DecisionError::InvalidRequest)?;
        let estimated = estimate_text_tokens(request.state);
        if estimated > STATE_BUDGET {
            return Err(DecisionError::StateOverBudget {
                estimated,
                budget: STATE_BUDGET,
            });
        }
        let body = serde_json::to_vec(&request_body(&self.model, request))
            .map_err(|error| DecisionError::Model(Box::new(error)))?;
        let text = crate::decision_http::post_json(
            &self.client,
            &self.url,
            self.api_key.as_ref(),
            body,
            cancellation,
        )
        .await?;
        // A parse error quotes the response, so it is replaced, not kept as
        // the source.
        let response: ResponseBody = serde_json::from_str(&text)
            .map_err(|_| invalid("the response is not an OpenAI decisions object".into()))?;
        answers(response, request.questions)
    }
}

impl std::fmt::Debug for OpenAiDecisionsModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiDecisionsModel")
            .field("model", &self.model)
            .field("has_api_key", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}

impl DecisionModel for OpenAiDecisionsModel {
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        cancellation: &'a CancellationToken,
    ) -> DecisionFuture<'a> {
        Box::pin(self.decide_now(request, cancellation))
    }

    fn state_budget(&self) -> Option<u64> {
        Some(STATE_BUDGET)
    }
}

fn invalid(message: String) -> DecisionError {
    DecisionError::InvalidResponse(message)
}

fn request_body<'a>(model: &'a str, request: DecisionRequest<'a>) -> RequestBody<'a> {
    let questions = request
        .questions
        .iter()
        .map(|question| {
            let mut instructions = format!("{}\n\n{}", request.instructions, question.instructions);
            let name = question.id;
            match question.kind {
                QuestionKind::Noul(criteria) => {
                    if let Some(criteria) = criteria {
                        instructions.push_str(&format!(
                            "\n\nTrue when: {}\nFalse when: {}",
                            criteria.when_true, criteria.when_false
                        ));
                    }
                    QuestionBody::Predicate { name, instructions }
                }
                QuestionKind::Choice(options) => QuestionBody::Choice {
                    name,
                    instructions,
                    choices: options
                        .iter()
                        .map(|option| ChoiceBody {
                            value: option.id,
                            description: option.description,
                        })
                        .collect(),
                },
                // A level is a description; its index labels it, as in the
                // text adapter's prompt.
                QuestionKind::Score(levels) => QuestionBody::Score {
                    name,
                    instructions,
                    levels: levels
                        .iter()
                        .enumerate()
                        .map(|(index, description)| LevelBody {
                            label: index.to_string(),
                            description,
                        })
                        .collect(),
                },
            }
        })
        .collect();
    RequestBody {
        model,
        input: request.state,
        questions,
    }
}

/// One answer per question, in question order, and no answer to anything
/// else. Every answer must be of its question's type, and choice and score
/// answers must give a distribution over every option or level.
fn answers(
    response: ResponseBody,
    questions: &[Question<'_>],
) -> Result<Vec<Answer>, DecisionError> {
    let mut by_name = HashMap::with_capacity(response.answers.len());
    for answer in response.answers {
        if by_name.insert(answer.name.clone(), answer).is_some() {
            return Err(invalid("two answers share a name".into()));
        }
    }
    let answers = questions
        .iter()
        .map(|question| {
            let id = question.id;
            let answer = by_name
                .remove(id)
                .ok_or_else(|| invalid(format!("no answer to `{id}`")))?;
            match (question.kind, answer.kind.as_str()) {
                (QuestionKind::Noul(_), "predicate") => answer
                    .probability
                    .and_then(NoulAnswer::from_probability)
                    .map(Answer::Noul),
                (QuestionKind::Choice(options), "choice") => {
                    let option = answer
                        .choice
                        .and_then(|choice| options.iter().position(|option| option.id == choice));
                    let probabilities = answer.probabilities.and_then(|probabilities| {
                        in_order(
                            probabilities.into_iter().map(|entry| {
                                Some((entry.value.as_str()?.to_owned(), entry.probability))
                            }),
                            options.iter().map(|option| option.id.to_owned()),
                        )
                    });
                    option
                        .zip(probabilities)
                        .and_then(|(option, probabilities)| {
                            ChoiceAnswer::from_probabilities(option, probabilities)
                        })
                        .map(Answer::Choice)
                }
                (QuestionKind::Score(levels), "score") => answer
                    .probabilities
                    .and_then(|probabilities| {
                        in_order(
                            probabilities.into_iter().map(|entry| {
                                let level = usize::try_from(entry.value.as_u64()?).ok()?;
                                Some((level, entry.probability))
                            }),
                            0..levels.len(),
                        )
                    })
                    .and_then(ScoreAnswer::from_probabilities)
                    .map(Answer::Score),
                (QuestionKind::Noul(_) | QuestionKind::Choice(_) | QuestionKind::Score(_), _) => {
                    None
                }
            }
            .ok_or_else(|| invalid(format!("the answer to `{id}` is unusable")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !by_name.is_empty() {
        return Err(invalid(format!(
            "{} answer(s) to questions not asked",
            by_name.len()
        )));
    }
    Ok(answers)
}

/// The probability of each of `keys`, in order, if `entries` are readable,
/// name each key exactly once, and name nothing else.
fn in_order<K: Eq + Hash>(
    entries: impl Iterator<Item = Option<(K, f64)>>,
    keys: impl ExactSizeIterator<Item = K>,
) -> Option<Vec<f64>> {
    let mut probabilities = HashMap::new();
    for entry in entries {
        let (key, probability) = entry?;
        if probabilities.insert(key, probability).is_some() {
            return None;
        }
    }
    if probabilities.len() != keys.len() {
        return None;
    }
    keys.map(|key| probabilities.get(&key).copied()).collect()
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    input: &'a str,
    questions: Vec<QuestionBody<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum QuestionBody<'a> {
    Predicate {
        name: &'a str,
        instructions: String,
    },
    Choice {
        name: &'a str,
        instructions: String,
        choices: Vec<ChoiceBody<'a>>,
    },
    Score {
        name: &'a str,
        instructions: String,
        levels: Vec<LevelBody<'a>>,
    },
}

#[derive(Serialize)]
struct ChoiceBody<'a> {
    value: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct LevelBody<'a> {
    label: String,
    description: &'a str,
}

#[derive(Deserialize)]
struct ResponseBody {
    answers: Vec<AnswerBody>,
}

/// One answer, flat rather than an internally tagged enum: serde buffers a
/// tagged enum's fields, which cannot read numbers when serde_json's
/// `arbitrary_precision` is on, as it is in the Rho binary. `confidence` and
/// a score's `score` are derived from `probabilities`, so they are not read.
#[derive(Deserialize)]
struct AnswerBody {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    probability: Option<f64>,
    choice: Option<String>,
    probabilities: Option<Vec<ProbabilityBody>>,
}

/// One option's or level's probability: `value` is a choice's value string
/// or a level's zero-based index.
#[derive(Deserialize)]
struct ProbabilityBody {
    value: serde_json::Value,
    probability: f64,
}

#[cfg(test)]
#[path = "openai_decisions_tests.rs"]
mod tests;
