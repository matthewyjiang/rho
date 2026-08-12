mod classify;
mod transcript;
mod verdict;

#[allow(unused_imports)] // consumed by the handler task in the permission classifier rollout
pub(crate) use classify::{classify_capability_request, ClassifyRequest};
pub(crate) use transcript::render_classifier_transcript;
pub(crate) use verdict::{parse_classifier_verdict, ClassifierVerdict, CLASSIFIER_PROMPT};

#[allow(dead_code)]
pub(crate) const CONSECUTIVE_DENY_ESCALATION: u32 = 3;

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod transcript_tests;

#[cfg(test)]
#[path = "verdict_tests.rs"]
mod verdict_tests;

#[cfg(test)]
#[path = "classify_tests.rs"]
mod classify_tests;
