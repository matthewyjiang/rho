//! Typed decision protocol: a state plus typed questions in, one answer per
//! question out.
//!
//! The shape follows the System One API (`POST /v1/systemone`) that decision
//! models such as TypeSafe's Jev, Cloudflare's Clef, and Ollama's decision
//! models serve, so a feature written against it can run on a decision model
//! or, through [`llm`], on any text model. Only `choice` questions exist so
//! far; the protocol's `noul` and `score` types land with their first user.

pub(crate) mod llm;
mod model;
mod resolve;
pub(crate) mod system_one;

pub(crate) use model::DecisionModel;
pub(crate) use resolve::{resolve, ConfigError};

use std::collections::HashMap;

/// One decision request. The state is evidence only; every rule the answer
/// must follow belongs in the instructions or the questions.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DecisionRequest<'a> {
    /// Rules shared by every question. The System One API has no field for
    /// them, so a decision-model backend prepends them to each question's
    /// instructions; the text-model adapter sends them in the system prompt.
    pub instructions: &'a str,
    pub state: &'a str,
    pub questions: &'a [ChoiceQuestion],
}

/// Most options a choice question may have. Ollama's System One server
/// accepts 2 to 26, the smallest cap among the published servers.
const MAX_CHOICE_OPTIONS: usize = 26;

/// Longest question or option ID. Workers AI rejects question IDs over 100
/// characters.
const MAX_ID_LEN: usize = 100;

/// A pick-one question. Option IDs are what the answer names; descriptions
/// tell the model what each option means.
///
/// Declare questions as constants checked by [`Self::validate`], so a
/// question no server would accept fails the build.
#[derive(Debug)]
pub(crate) struct ChoiceQuestion {
    /// Answers come back under this ID.
    pub id: &'static str,
    pub instructions: &'static str,
    pub options: &'static [ChoiceOption],
}

impl ChoiceQuestion {
    /// Panics, at compile time when called in a `const` item, unless the
    /// question has valid unique IDs and 2 to [`MAX_CHOICE_OPTIONS`] options.
    pub(crate) const fn validate(&self) {
        assert!(valid_id(self.id), "invalid question ID");
        assert!(
            self.options.len() >= 2 && self.options.len() <= MAX_CHOICE_OPTIONS,
            "a choice question needs 2 to 26 options"
        );
        let mut index = 0;
        while index < self.options.len() {
            assert!(valid_id(self.options[index].id), "invalid option ID");
            let mut earlier = 0;
            while earlier < index {
                assert!(
                    !bytes_eq(self.options[earlier].id, self.options[index].id),
                    "duplicate option ID"
                );
                earlier += 1;
            }
            index += 1;
        }
    }
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ChoiceOption {
    pub id: &'static str,
    pub description: &'static str,
}

/// One question's answer: the chosen option and, from a model that reports
/// them, its probability for each option.
#[derive(Debug, PartialEq)]
pub(crate) struct Answer {
    pub option: &'static ChoiceOption,
    /// `None` from a model that reports no probabilities, such as a text
    /// model; never a stand-in of 1.0 for the chosen option.
    pub probabilities: Option<HashMap<&'static str, f64>>,
}

impl Answer {
    /// The model's probability for `option_id`, zero when it reported none
    /// for that option; `None` when the model reports no probabilities.
    pub(crate) fn probability(&self, option_id: &str) -> Option<f64> {
        let probabilities = self.probabilities.as_ref()?;
        Some(probabilities.get(option_id).copied().unwrap_or(0.0))
    }
}

/// The answer to each question, keyed by question ID.
pub(crate) type Answers = HashMap<&'static str, Answer>;
