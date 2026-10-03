//! Decision-model backend: asks a [`DecisionRequest`] of a model served over
//! the System One API (`POST {host}/v1/systemone`), such as Clef on Ollama.
//!
//! The API has no field for shared instructions, so each question's
//! instructions start with the request's. The answer to each question is the
//! chosen option and the model's probability for every option.
//!
//! Errors name only the HTTP status and the request's own IDs, never text
//! from the response, which a server or proxy may fill with the request's
//! credentials.

use std::{collections::HashMap, time::Duration};

use anyhow::{bail, Context};
use futures_util::future::BoxFuture;
use rho_sdk::{model::context::estimate_text_tokens, CancellationToken};
use serde::{ser::SerializeMap, Deserialize, Serialize, Serializer};
use url::Url;

use super::{Answer, Answers, ChoiceOption, ChoiceQuestion, DecisionModel, DecisionRequest};

/// Largest request body sent. Ollama answers 413 "request body must not
/// exceed 64 KiB without images" above it, so a larger request fails here
/// with both sizes instead.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Tripwire for one decision. Measured on clef-flash: 9.9 s for a cold
/// model load, then 4.3 s for a 15.6k-token state, the largest its default
/// 16,384-token context accepts.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Largest state, in Rho's chars/4 estimate, for Clef at Ollama's default
/// 16,384-token context with up to 512 tokens of instructions and questions.
///
/// Ollama rejects, never truncates, input over the context. Measured on 84
/// permission-classifier requests on clef: about 430 tokens of instructions,
/// plus 0.30 to 0.37 tokens per state char, where the estimate assumes 0.25.
/// So (16,384 - 512) / 1.5 = 10,581.
const STATE_BUDGET_TOKENS: u64 = 10_500;

/// A decision model on a System One server.
pub(crate) struct SystemOneModel {
    client: reqwest::Client,
    url: Url,
    model: String,
    api_key: Option<String>,
}

impl SystemOneModel {
    /// `api_base` is the server's OpenAI-compatible base, `{host}/v1`, as
    /// configured for the provider. An `api_key` is sent as a bearer token.
    pub(crate) fn new(
        api_base: &Url,
        model: String,
        api_key: Option<String>,
    ) -> anyhow::Result<Self> {
        let mut url = api_base.clone();
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("{api_base} cannot be a System One base URL"))?
            .pop_if_empty()
            .push("systemone");
        let client = crate::reqwest_client_builder()
            .timeout(TIMEOUT)
            .build()
            .context("failed to build System One HTTP client")?;
        Ok(Self {
            client,
            url,
            model,
            api_key,
        })
    }

    async fn decide_now(
        &self,
        request: DecisionRequest<'_>,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<Answers> {
        let state_tokens = estimate_text_tokens(request.state);
        if state_tokens > STATE_BUDGET_TOKENS {
            bail!(
                "decision state needs ~{state_tokens} tokens; the System One state budget is {STATE_BUDGET_TOKENS}"
            );
        }
        let body = request_body(&self.model, request)?;
        let response = async {
            let mut post = self
                .client
                .post(self.url.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
            if let Some(api_key) = &self.api_key {
                post = post.bearer_auth(api_key);
            }
            let response = post.send().await?;
            let status = response.status();
            let text = response.text().await?;
            reqwest::Result::Ok((status, text))
        };
        let (status, text) = tokio::select! {
            () = cancellation.cancelled() => bail!("decision request cancelled"),
            // Without the URL, which may carry credentials of its own.
            response = response => response
                .map_err(reqwest::Error::without_url)
                .context("System One request failed")?,
        };
        if !status.is_success() {
            bail!("System One server returned {status}");
        }
        // A parse error quotes the response, so it is replaced, not kept as
        // the source.
        let response: ResponseBody = serde_json::from_str(&text)
            .map_err(|_| anyhow::anyhow!("System One server returned an unreadable response"))?;
        answers(response, request.questions)
    }
}

impl DecisionModel for SystemOneModel {
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        cancellation: &'a CancellationToken,
    ) -> BoxFuture<'a, anyhow::Result<Answers>> {
        Box::pin(self.decide_now(request, cancellation))
    }

    fn state_budget(&self) -> Option<u64> {
        Some(STATE_BUDGET_TOKENS)
    }
}

/// The JSON body for `request`, or an error naming both sizes when it is
/// over [`MAX_BODY_BYTES`].
fn request_body(model: &str, request: DecisionRequest<'_>) -> anyhow::Result<Vec<u8>> {
    let body = serde_json::to_vec(&RequestBody {
        model,
        state: request.state,
        questions: Questions(request),
    })?;
    if body.len() > MAX_BODY_BYTES {
        bail!(
            "decision request body is {} bytes; the System One limit is {MAX_BODY_BYTES} bytes",
            body.len()
        );
    }
    Ok(body)
}

fn answers(mut response: ResponseBody, questions: &[ChoiceQuestion]) -> anyhow::Result<Answers> {
    questions
        .iter()
        .map(|question| {
            let answer = response
                .answers
                .remove(question.id)
                .with_context(|| format!("no answer to `{}`", question.id))?;
            let option = question
                .options
                .iter()
                .find(|option| option.id == answer.choice)
                .with_context(|| {
                    format!("the answer to `{}` is not one of its options", question.id)
                })?;
            // An out-of-range probability is a broken answer, never a
            // stronger one.
            if answer
                .probabilities
                .values()
                .any(|probability| !(0.0..=1.0).contains(probability))
            {
                bail!(
                    "a probability in the answer to `{}` is outside 0 to 1",
                    question.id
                );
            }
            let probabilities = question
                .options
                .iter()
                .filter_map(|option| {
                    let probability = *answer.probabilities.get(option.id)?;
                    Some((option.id, probability))
                })
                .collect();
            Ok((
                question.id,
                Answer {
                    option,
                    probabilities: Some(probabilities),
                },
            ))
        })
        .collect()
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
            map.serialize_entry(
                question.id,
                &QuestionBody {
                    kind: "choice",
                    instructions: format!("{}\n\n{}", self.0.instructions, question.instructions),
                    criteria: Criteria(question.options),
                },
            )?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct QuestionBody {
    #[serde(rename = "type")]
    kind: &'static str,
    instructions: String,
    criteria: Criteria,
}

/// A choice question's options as the API's `{id: description}` map, in order.
struct Criteria(&'static [ChoiceOption]);

impl Serialize for Criteria {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|option| (option.id, option.description)))
    }
}

#[derive(Deserialize)]
struct ResponseBody {
    answers: HashMap<String, AnswerBody>,
}

#[derive(Deserialize)]
struct AnswerBody {
    choice: String,
    #[serde(default)]
    probabilities: HashMap<String, f64>,
}

#[cfg(test)]
#[path = "system_one/test_server.rs"]
pub(crate) mod test_server;

#[cfg(test)]
#[path = "system_one_tests.rs"]
mod tests;
