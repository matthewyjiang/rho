//! A [`DecisionModel`] served over the System One API
//! (`POST {api_base}/systemone`), such as Clef or Jev.
//!
//! The API has no field for a request's shared instructions, so each
//! question's instructions start with them. Errors name only the HTTP status,
//! sizes and limits, and the request's own IDs, never text from the response,
//! which a server or proxy may fill with the request's credentials.

use std::{collections::HashMap, time::Duration};

use rho_sdk::{
    decision::{
        Answer, ChoiceAnswer, ChoiceOption, DecisionError, DecisionFuture, DecisionModel,
        DecisionRequest, NoulAnswer, NoulCriteria, Question, QuestionKind, ScoreAnswer,
    },
    model::context::estimate_text_tokens,
    CancellationToken, SecretString,
};
use serde::{ser::SerializeMap, Deserialize, Serialize, Serializer};
use url::Url;

/// Tripwire for one decision. Measured on clef-flash on Ollama: 9.9 s for a
/// cold model load, then 4.3 s for a 15.6k-token state, the largest its
/// default 16,384-token context accepts.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What one server accepts. A request over a limit fails before it is sent,
/// naming the limit and the asked size, instead of with the server's error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemOneLimits {
    /// Largest request body the server accepts.
    pub max_body_bytes: Option<usize>,
    /// Largest state, in
    /// [`estimate_text_tokens`](rho_sdk::model::context::estimate_text_tokens).
    pub state_budget: Option<u64>,
}

impl SystemOneLimits {
    /// Ollama with a decision model at its default 16,384-token context.
    ///
    /// Ollama answers 413 "request body must not exceed 64 KiB without
    /// images" over 64 KiB, and rejects, never truncates, input over the
    /// model's context. Measured on 84 permission-classifier requests on
    /// clef: about 430 tokens of instructions and questions, plus 0.30 to 0.37
    /// tokens per state char, where the estimate assumes 0.25. So the state
    /// budget is (16,384 - 512) / 1.5 = 10,581, rounded down.
    pub const OLLAMA: Self = Self {
        max_body_bytes: Some(64 * 1024),
        state_budget: Some(10_500),
    };
}

/// A decision model on a System One server.
pub struct SystemOneModel {
    client: reqwest::Client,
    url: Url,
    model: String,
    api_key: Option<SecretString>,
    limits: SystemOneLimits,
}

/// Why a [`SystemOneModel`] could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SystemOneSetupError {
    #[error("the System One base URL cannot take a path")]
    BaseUrl,
    #[error("failed to build the System One HTTP client")]
    Client(#[source] reqwest::Error),
}

impl SystemOneModel {
    /// `api_base` is the server's API base, such as `http://localhost:11434/v1`
    /// for Ollama or `https://api.typesafe.ai/v1`; requests go to
    /// `{api_base}/systemone`. An `api_key` is sent as a bearer token. The
    /// model has no limits until [`Self::with_limits`].
    pub fn new(
        api_base: &Url,
        model: impl Into<String>,
        api_key: Option<SecretString>,
    ) -> Result<Self, SystemOneSetupError> {
        let mut url = api_base.clone();
        url.path_segments_mut()
            .map_err(|()| SystemOneSetupError::BaseUrl)?
            .pop_if_empty()
            .push("systemone");
        let client = crate::tls::reqwest_client_builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| SystemOneSetupError::Client(error.without_url()))?;
        Ok(Self {
            client,
            url,
            model: model.into(),
            api_key,
            limits: SystemOneLimits::default(),
        })
    }

    pub fn with_limits(mut self, limits: SystemOneLimits) -> Self {
        self.limits = limits;
        self
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
        if let Some(budget) = self.limits.state_budget {
            let estimated = estimate_text_tokens(request.state);
            if estimated > budget {
                return Err(DecisionError::StateOverBudget { estimated, budget });
            }
        }
        let body = request_body(&self.model, request, self.limits.max_body_bytes)?;
        let response = async {
            let mut post = self
                .client
                .post(self.url.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
            if let Some(api_key) = &self.api_key {
                post = post.bearer_auth(api_key.expose_secret());
            }
            let response = post.send().await?;
            let status = response.status();
            let text = response.text().await?;
            reqwest::Result::Ok((status, text))
        };
        let (status, text) = tokio::select! {
            () = cancellation.cancelled() => return Err(DecisionError::Cancelled),
            // Without the URL, which may carry credentials of its own.
            response = response => response
                .map_err(|error| DecisionError::Model(Box::new(error.without_url())))?,
        };
        if !status.is_success() {
            return Err(DecisionError::Status {
                status: status.as_u16(),
            });
        }
        // A parse error quotes the response, so it is replaced, not kept as
        // the source.
        let response: ResponseBody = serde_json::from_str(&text)
            .map_err(|_| invalid("the response is not a System One answer object".into()))?;
        answers(response, request.questions)
    }
}

impl std::fmt::Debug for SystemOneModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SystemOneModel")
            .field("model", &self.model)
            .field("has_api_key", &self.api_key.is_some())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DecisionModel for SystemOneModel {
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        cancellation: &'a CancellationToken,
    ) -> DecisionFuture<'a> {
        Box::pin(self.decide_now(request, cancellation))
    }

    fn state_budget(&self) -> Option<u64> {
        self.limits.state_budget
    }
}

fn invalid(message: String) -> DecisionError {
    DecisionError::InvalidResponse(message)
}

/// The JSON body for `request`, or an error naming both sizes when it is
/// over `max_bytes`.
fn request_body(
    model: &str,
    request: DecisionRequest<'_>,
    max_bytes: Option<usize>,
) -> Result<Vec<u8>, DecisionError> {
    let body = serde_json::to_vec(&RequestBody {
        model,
        state: request.state,
        questions: Questions(request),
    })
    .map_err(|error| DecisionError::Model(Box::new(error)))?;
    match max_bytes {
        Some(limit) if body.len() > limit => Err(DecisionError::RequestTooLarge {
            bytes: body.len(),
            limit,
        }),
        Some(_) | None => Ok(body),
    }
}

/// One answer per question, in question order, and no answer to anything
/// else. Every answer must be of its question's type, and must give a
/// distribution over every possible answer.
fn answers(
    response: ResponseBody,
    questions: &[Question<'_>],
) -> Result<Vec<Answer>, DecisionError> {
    let mut response = response.answers.0;
    let answers = questions
        .iter()
        .map(|question| {
            let id = question.id;
            let answer = response
                .remove(id)
                .ok_or_else(|| invalid(format!("no answer to `{id}`")))?;
            let unusable = || invalid(format!("the answer to `{id}` is unusable"));
            match (question.kind, answer.kind.as_str()) {
                (QuestionKind::Noul(_), "noul") => answer
                    .noul
                    .and_then(NoulAnswer::from_probability)
                    .map(Answer::Noul),
                (QuestionKind::Choice(options), "choice") => {
                    let option = answer
                        .choice
                        .and_then(|choice| options.iter().position(|option| option.id == choice));
                    let probabilities = answer.probabilities.and_then(|probabilities| {
                        in_order(
                            &probabilities.0,
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
                            &probabilities.0,
                            (0..levels.len()).map(|level| level.to_string()),
                        )
                    })
                    .and_then(ScoreAnswer::from_probabilities)
                    .map(Answer::Score),
                (QuestionKind::Noul(_) | QuestionKind::Choice(_) | QuestionKind::Score(_), _) => {
                    None
                }
            }
            .ok_or_else(unusable)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !response.is_empty() {
        return Err(invalid(format!(
            "{} answer(s) to questions not asked",
            response.len()
        )));
    }
    Ok(answers)
}

/// The probability of each of `keys`, in order, if the map has exactly them.
fn in_order(
    probabilities: &HashMap<String, f64>,
    keys: impl ExactSizeIterator<Item = String>,
) -> Option<Vec<f64>> {
    if probabilities.len() != keys.len() {
        return None;
    }
    keys.map(|key| probabilities.get(&key).copied()).collect()
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    state: &'a str,
    questions: Questions<'a>,
}

/// The request's questions as the API's `{id: question}` map, in order.
struct Questions<'a>(DecisionRequest<'a>);

impl Serialize for Questions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.questions.len()))?;
        for question in self.0.questions {
            let instructions = format!("{}\n\n{}", self.0.instructions, question.instructions);
            let body = match question.kind {
                QuestionKind::Noul(criteria) => QuestionBody::Noul {
                    instructions,
                    criteria: criteria.map(NoulBody),
                },
                QuestionKind::Choice(options) => QuestionBody::Choice {
                    instructions,
                    criteria: ChoiceBody(options),
                },
                QuestionKind::Score(levels) => QuestionBody::Score {
                    instructions,
                    criteria: levels,
                },
            };
            map.serialize_entry(question.id, &body)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum QuestionBody<'a> {
    Noul {
        instructions: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulBody<'a>>,
    },
    Choice {
        instructions: String,
        criteria: ChoiceBody<'a>,
    },
    Score {
        instructions: String,
        criteria: &'a [&'a str],
    },
}

/// A noul question's criteria as the API's `{"true": .., "false": ..}`.
struct NoulBody<'a>(NoulCriteria<'a>);

impl Serialize for NoulBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("true", self.0.when_true)?;
        map.serialize_entry("false", self.0.when_false)?;
        map.end()
    }
}

/// A choice question's options as the API's `{id: description}` map, in
/// order.
struct ChoiceBody<'a>(&'a [ChoiceOption<'a>]);

impl Serialize for ChoiceBody<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|option| (option.id, option.description)))
    }
}

#[derive(Deserialize)]
struct ResponseBody {
    answers: UniqueMap<AnswerBody>,
}

/// One answer, flat rather than an internally tagged enum: serde buffers a
/// tagged enum's fields, which cannot read numbers when serde_json's
/// `arbitrary_precision` is on, as it is in the Rho binary.
#[derive(Deserialize)]
struct AnswerBody {
    #[serde(rename = "type")]
    kind: String,
    noul: Option<f64>,
    choice: Option<String>,
    probabilities: Option<UniqueMap<f64>>,
}

/// A JSON object that fails to deserialize when a key repeats, which a map
/// would resolve by keeping one value unchecked. Keys compare after JSON
/// unescaping, so an escaped repeat is a repeat too.
struct UniqueMap<V>(HashMap<String, V>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for UniqueMap<V> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<V>(std::marker::PhantomData<V>);
        impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<V> {
            type Value = UniqueMap<V>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object with unique keys")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueMap<V>, A::Error> {
                let mut entries = HashMap::new();
                while let Some((key, value)) = map.next_entry::<String, V>()? {
                    if entries.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("an object key repeats"));
                    }
                }
                Ok(UniqueMap(entries))
            }
        }
        deserializer.deserialize_map(Visitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
#[path = "system_one/test_server.rs"]
mod test_server;

#[cfg(test)]
#[path = "system_one_tests.rs"]
mod tests;
