mod classify;
mod transcript;
mod verdict;

pub(crate) use classify::{classify_capability_request, ClassifyRequest};
pub(crate) use transcript::render_classifier_transcript;
pub(crate) use verdict::{
    parse_classifier_verdict, parse_screen_verdict, ClassifierVerdict, ScreenVerdict,
    CLASSIFIER_PROMPT, CLASSIFIER_REVIEW_INSTRUCTION, CLASSIFIER_SCREEN_INSTRUCTION,
};

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod transcript_tests;

#[cfg(test)]
#[path = "verdict_tests.rs"]
mod verdict_tests;

#[cfg(test)]
#[path = "classify_tests.rs"]
mod classify_tests;
