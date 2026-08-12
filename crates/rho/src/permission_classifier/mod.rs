mod classify;
mod transcript;
mod verdict;

pub(crate) use classify::{classify_capability_request, ClassifyRequest};
pub(crate) use transcript::render_classifier_transcript;
pub(crate) use verdict::{parse_classifier_verdict, ClassifierVerdict, CLASSIFIER_PROMPT};

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod transcript_tests;

#[cfg(test)]
#[path = "verdict_tests.rs"]
mod verdict_tests;

#[cfg(test)]
#[path = "classify_tests.rs"]
mod classify_tests;
