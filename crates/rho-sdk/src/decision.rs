//! Typed decisions: a state plus typed questions in, one answer per question
//! out.
//!
//! The shape follows the System One API (`POST /v1/systemone`) that decision
//! models such as TypeSafe's Jev and Cloudflare's Clef serve, locally through
//! Ollama or hosted. A feature written against [`DecisionModel`] runs on a
//! decision model or, through [`text::TextDecisionModel`], on any text model
//! behind a [`ModelProvider`](crate::provider::ModelProvider).
//!
//! A request asks three kinds of question:
//!
//! - [`QuestionKind::Noul`]: yes or no.
//! - [`QuestionKind::Choice`]: one of several named options.
//! - [`QuestionKind::Score`]: a level on an ordered scale.
//!
//! A decision model also reports its probability for every possible answer.
//! A text model reports none, and its answers say so instead of inventing
//! certainty.
//!
//! The core crate has no network client. Rho's System One client lives in
//! `rho-providers`.
//!
//! ```
//! use rho_sdk::decision::{ChoiceOption, DecisionRequest, Question};
//!
//! const TEAM: Question<'static> = Question::choice(
//!     "team",
//!     "Which team should handle this request?",
//!     &[
//!         ChoiceOption::new("billing", "Payments, invoices, and refunds"),
//!         ChoiceOption::new("technical", "Outages, errors, and configuration"),
//!     ],
//! );
//! // Fails the build if a published System One server would reject it.
//! const _: () = assert!(TEAM.check().is_ok());
//!
//! let questions = [TEAM];
//! let request = DecisionRequest::new(
//!     "You route support tickets.",
//!     "Checkout has failed for every customer since 9am.",
//!     &questions,
//! );
//! assert!(request.check().is_ok());
//! ```

pub mod text;

use std::{future::Future, pin::Pin};

use crate::CancellationToken;

/// One decision request. The state is evidence only; every rule the answers
/// must follow belongs in the instructions or the questions.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct DecisionRequest<'a> {
    /// Rules shared by every question. The System One API has no field for
    /// them, so a decision-model client prepends them to each question's
    /// instructions; the text-model adapter sends them as the system prompt.
    pub instructions: &'a str,
    /// What the questions are about.
    pub state: &'a str,
    pub questions: &'a [Question<'a>],
}

impl<'a> DecisionRequest<'a> {
    pub const fn new(instructions: &'a str, state: &'a str, questions: &'a [Question<'a>]) -> Self {
        Self {
            instructions,
            state,
            questions,
        }
    }

    /// Checks the request against the smallest limits of the published
    /// System One servers, so a request that passes runs on any of them.
    pub const fn check(&self) -> Result<(), InvalidRequest> {
        let count = self.questions.len();
        if count == 0 || count > MAX_QUESTIONS {
            return Err(InvalidRequest::QuestionCount);
        }
        let mut index = 0;
        while index < count {
            if let Err(error) = self.questions[index].check() {
                return Err(error);
            }
            let mut earlier = 0;
            while earlier < index {
                if bytes_eq(self.questions[earlier].id, self.questions[index].id) {
                    return Err(InvalidRequest::DuplicateQuestionId);
                }
                earlier += 1;
            }
            index += 1;
        }
        Ok(())
    }

    /// Checks that `answers` answer this request: one per question, in
    /// order, each of its question's kind, choosing an option or level the
    /// question has, with a probability for each of them when the model
    /// reports probabilities. Callers that take a [`DecisionModel`] they did
    /// not write check its answers here before acting on them.
    pub fn check_answers(&self, answers: &[Answer]) -> Result<(), DecisionError> {
        if answers.len() != self.questions.len() {
            return Err(DecisionError::InvalidResponse(format!(
                "{} answer(s) to {} question(s)",
                answers.len(),
                self.questions.len()
            )));
        }
        for (question, answer) in self.questions.iter().zip(answers) {
            let fits = match (question.kind, answer) {
                (QuestionKind::Noul(_), Answer::Noul(_)) => true,
                (QuestionKind::Choice(options), Answer::Choice(answer)) => {
                    answer.option < options.len()
                        && answer
                            .probabilities()
                            .is_none_or(|probabilities| probabilities.len() == options.len())
                }
                // With probabilities, the value is a weighted level that the
                // distribution tolerance can lift just past the top level, so
                // only a whole level is bounded.
                (QuestionKind::Score(levels), Answer::Score(answer)) => {
                    match answer.probabilities() {
                        Some(probabilities) => probabilities.len() == levels.len(),
                        None => answer.value <= (levels.len() - 1) as f64,
                    }
                }
                (
                    QuestionKind::Noul(_) | QuestionKind::Choice(_) | QuestionKind::Score(_),
                    Answer::Noul(_) | Answer::Choice(_) | Answer::Score(_),
                ) => false,
            };
            if !fits {
                return Err(DecisionError::InvalidResponse(format!(
                    "the answer to `{}` does not fit the question",
                    question.id
                )));
            }
        }
        Ok(())
    }
}

/// Most questions in one request: Cloudflare's Workers AI and Ollama both
/// accept at most 64.
pub const MAX_QUESTIONS: usize = 64;

/// Most options a choice question may have: Ollama accepts 2 to 26, the
/// smallest cap among the published servers (TypeSafe accepts 255).
pub const MAX_CHOICE_OPTIONS: usize = 26;

/// Most levels a score question may have: TypeSafe accepts 2 to 10, the
/// smallest cap among the published servers (Ollama accepts 26).
pub const MAX_SCORE_LEVELS: usize = 10;

/// Longest question or option ID: Workers AI rejects question IDs over 100
/// characters.
pub const MAX_ID_LEN: usize = 100;

/// One question. Its ID keys nothing on the wire but the answer; the
/// instructions say what to decide.
///
/// Declare fixed questions as constants and check them in a `const` item,
/// so a question no server would accept fails the build.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Question<'a> {
    pub id: &'a str,
    pub instructions: &'a str,
    pub kind: QuestionKind<'a>,
}

/// What a question asks for, with the descriptions that define its answers.
#[derive(Clone, Copy, Debug)]
pub enum QuestionKind<'a> {
    /// Yes or no, with optional descriptions of each.
    Noul(Option<NoulCriteria<'a>>),
    /// One of 2 to [`MAX_CHOICE_OPTIONS`] options.
    Choice(&'a [ChoiceOption<'a>]),
    /// One of 2 to [`MAX_SCORE_LEVELS`] level descriptions, lowest first.
    Score(&'a [&'a str]),
}

/// What a yes and a no mean for a noul question.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct NoulCriteria<'a> {
    pub when_true: &'a str,
    pub when_false: &'a str,
}

impl<'a> NoulCriteria<'a> {
    pub const fn new(when_true: &'a str, when_false: &'a str) -> Self {
        Self {
            when_true,
            when_false,
        }
    }
}

/// One option of a choice question. The answer names its ID; the
/// description tells the model what the option means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChoiceOption<'a> {
    pub id: &'a str,
    pub description: &'a str,
}

impl<'a> ChoiceOption<'a> {
    pub const fn new(id: &'a str, description: &'a str) -> Self {
        Self { id, description }
    }
}

impl<'a> Question<'a> {
    /// A yes-or-no question.
    pub const fn noul(
        id: &'a str,
        instructions: &'a str,
        criteria: Option<NoulCriteria<'a>>,
    ) -> Self {
        Self {
            id,
            instructions,
            kind: QuestionKind::Noul(criteria),
        }
    }

    /// A pick-one question.
    pub const fn choice(
        id: &'a str,
        instructions: &'a str,
        options: &'a [ChoiceOption<'a>],
    ) -> Self {
        Self {
            id,
            instructions,
            kind: QuestionKind::Choice(options),
        }
    }

    /// A rating on `levels`, lowest first.
    pub const fn score(id: &'a str, instructions: &'a str, levels: &'a [&'a str]) -> Self {
        Self {
            id,
            instructions,
            kind: QuestionKind::Score(levels),
        }
    }

    /// Checks the question against the smallest limits of the published
    /// System One servers.
    pub const fn check(&self) -> Result<(), InvalidRequest> {
        if !valid_id(self.id) {
            return Err(InvalidRequest::QuestionId);
        }
        match self.kind {
            QuestionKind::Noul(_) => Ok(()),
            QuestionKind::Choice(options) => {
                if options.len() < 2 || options.len() > MAX_CHOICE_OPTIONS {
                    return Err(InvalidRequest::OptionCount);
                }
                let mut index = 0;
                while index < options.len() {
                    if !valid_id(options[index].id) {
                        return Err(InvalidRequest::OptionId);
                    }
                    let mut earlier = 0;
                    while earlier < index {
                        if bytes_eq(options[earlier].id, options[index].id) {
                            return Err(InvalidRequest::DuplicateOptionId);
                        }
                        earlier += 1;
                    }
                    index += 1;
                }
                Ok(())
            }
            QuestionKind::Score(levels) => {
                if levels.len() < 2 || levels.len() > MAX_SCORE_LEVELS {
                    return Err(InvalidRequest::LevelCount);
                }
                Ok(())
            }
        }
    }
}

/// Why a request or question would be rejected by some System One server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InvalidRequest {
    #[error("a decision request needs 1 to {MAX_QUESTIONS} questions")]
    QuestionCount,
    #[error("two questions share an ID")]
    DuplicateQuestionId,
    #[error("a question ID must be 1 to {MAX_ID_LEN} ASCII letters, digits, `_`, `.`, or `-`")]
    QuestionId,
    #[error("a choice question needs 2 to {MAX_CHOICE_OPTIONS} options")]
    OptionCount,
    #[error("an option ID must be 1 to {MAX_ID_LEN} ASCII letters, digits, `_`, `.`, or `-`")]
    OptionId,
    #[error("two options of a choice question share an ID")]
    DuplicateOptionId,
    #[error("a score question needs 2 to {MAX_SCORE_LEVELS} levels")]
    LevelCount,
}

/// 1 to [`MAX_ID_LEN`] ASCII letters, digits, `_`, `.`, or `-`, the ID
/// alphabet Workers AI documents.
const fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_ID_LEN {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')) {
            return false;
        }
        index += 1;
    }
    true
}

const fn bytes_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// The answer to one question, of the question's kind.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

/// A yes or no, with the model's probability of yes when it reports one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoulAnswer {
    value: bool,
    probability: Option<f64>,
}

impl NoulAnswer {
    /// A model's yes or no, without a probability.
    pub const fn from_value(value: bool) -> Self {
        Self {
            value,
            probability: None,
        }
    }

    /// A model's probability of yes; `None` unless it is within 0 to 1.
    pub fn from_probability(probability: f64) -> Option<Self> {
        valid_probability(probability).then_some(Self {
            value: probability >= 0.5,
            probability: Some(probability),
        })
    }

    /// The more likely answer: yes at a probability of 0.5 or more.
    pub const fn value(&self) -> bool {
        self.value
    }

    /// The probability of yes, from a model that reports one.
    pub const fn probability(&self) -> Option<f64> {
        self.probability
    }
}

/// The chosen option, by index into the question's options, with the
/// model's probability for every option when it reports them.
#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceAnswer {
    option: usize,
    probabilities: Option<Vec<f64>>,
}

impl ChoiceAnswer {
    /// A model's choice, without probabilities.
    pub const fn from_option(option: usize) -> Self {
        Self {
            option,
            probabilities: None,
        }
    }

    /// A model's choice with its probability for each option, in option
    /// order; `None` unless they are a distribution (each within 0 to 1,
    /// totaling 1 within 0.005 per probability) and `option` has the highest
    /// probability, tied or not. Rounding never reorders probabilities, so a
    /// server that rounds still passes.
    pub fn from_probabilities(option: usize, probabilities: Vec<f64>) -> Option<Self> {
        let chosen = *probabilities.get(option)?;
        (valid_distribution(&probabilities)
            && probabilities
                .iter()
                .all(|&probability| probability <= chosen))
        .then_some(Self {
            option,
            probabilities: Some(probabilities),
        })
    }

    /// Index of the chosen option.
    pub const fn option(&self) -> usize {
        self.option
    }

    /// The probability of each option, in option order, from a model that
    /// reports them.
    pub fn probabilities(&self) -> Option<&[f64]> {
        self.probabilities.as_deref()
    }

    /// The probability of the option at `index`, from a model that reports
    /// probabilities.
    pub fn probability(&self, index: usize) -> Option<f64> {
        self.probabilities()?.get(index).copied()
    }
}

/// A level on the question's scale: a whole level from a text model, or the
/// probability-weighted level from a decision model, which can fall between
/// levels.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreAnswer {
    value: f64,
    probabilities: Option<Vec<f64>>,
}

impl ScoreAnswer {
    /// A model's chosen level, without probabilities.
    pub fn from_level(level: usize) -> Self {
        Self {
            value: level as f64,
            probabilities: None,
        }
    }

    /// A model's probability for each level, lowest first; `None` unless
    /// they are a distribution: each within 0 to 1, totaling 1 within 0.005
    /// per probability. The value is the sum of each zero-based level times
    /// its probability.
    pub fn from_probabilities(probabilities: Vec<f64>) -> Option<Self> {
        if !valid_distribution(&probabilities) {
            return None;
        }
        let value = probabilities
            .iter()
            .enumerate()
            .map(|(level, probability)| level as f64 * probability)
            .sum();
        Some(Self {
            value,
            probabilities: Some(probabilities),
        })
    }

    /// The zero-based level.
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// The probability of each level, lowest first, from a model that
    /// reports them.
    pub fn probabilities(&self) -> Option<&[f64]> {
        self.probabilities.as_deref()
    }
}

fn valid_probability(probability: f64) -> bool {
    (0.0..=1.0).contains(&probability)
}

/// How far, per probability, a distribution's total may stray from 1.
///
/// Probabilities over a question's answers must each be within 0 to 1 and
/// total 1, or one broken response could make every answer certain. TypeSafe
/// rounds each probability to 2 decimals, off by up to 0.005 each, so a
/// total of `n` probabilities may be off by `0.005 * n`; full-precision
/// servers such as Ollama stay within floating-point error.
const DISTRIBUTION_TOLERANCE: f64 = 0.005;

fn valid_distribution(probabilities: &[f64]) -> bool {
    let total: f64 = probabilities.iter().sum();
    !probabilities.is_empty()
        && probabilities.iter().copied().all(valid_probability)
        && (total - 1.0).abs() <= DISTRIBUTION_TOLERANCE * probabilities.len() as f64
}

/// Future returned by [`DecisionModel::decide`].
pub type DecisionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Answer>, DecisionError>> + Send + 'a>>;

/// Answers [`DecisionRequest`]s: a decision model over the System One API,
/// or a text model through [`text::TextDecisionModel`].
///
/// Implementors return one answer per question, in question order and of
/// the question's kind, or an error; never a default answer, so a caller can
/// fail closed. A model that reports probabilities builds answers with the
/// `from_probabilities` constructors, one that does not with `from_value`,
/// `from_option`, or `from_level`. Errors never repeat response text or
/// credentials. The returned future must observe `cancellation`.
/// [`DecisionRequest::check_answers`] checks the answer shape for callers.
pub trait DecisionModel: Send + Sync {
    /// Answers `request`.
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        cancellation: &'a CancellationToken,
    ) -> DecisionFuture<'a>;

    /// Largest state the model accepts, in
    /// [`estimate_text_tokens`](crate::model::context::estimate_text_tokens),
    /// or `None` when the caller sizes the state for the model. A caller with
    /// a longer state shortens it first.
    fn state_budget(&self) -> Option<u64> {
        None
    }
}

/// Why a decision failed. Messages name sizes, limits, statuses, and the
/// request's own IDs, never response text or credentials.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DecisionError {
    #[error("decision request is invalid: {0}")]
    InvalidRequest(InvalidRequest),
    #[error("decision request cancelled")]
    Cancelled,
    #[error("decision state needs ~{estimated} tokens; the model's state budget is {budget}")]
    StateOverBudget { estimated: u64, budget: u64 },
    #[error("decision request body is {bytes} bytes; the server limit is {limit} bytes")]
    RequestTooLarge { bytes: usize, limit: usize },
    #[error("decision server returned HTTP {status}")]
    Status { status: u16 },
    /// The response could not be read as answers to the questions asked.
    /// The message must not repeat the response.
    #[error("decision response is unusable: {0}")]
    InvalidResponse(String),
    /// The model or its transport failed; the source carries the details.
    #[error("decision model failed")]
    Model(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
