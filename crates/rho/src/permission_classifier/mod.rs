mod budget;
mod classify;
mod transcript;
mod verdict;

use budget::{TranscriptBudget, TranscriptOverBudget};
pub(crate) use classify::{
    check_screen_config, classify_capability_request, screen_warning, ClassifierModel,
    ClassifyRequest, ScreenOutcome, DECISION_SCREEN_ID,
};
#[cfg(test)]
pub(crate) use transcript::render_classifier_transcript;
pub(crate) use verdict::{
    review_verdict, screen_allow_probability, screen_verdict, ClassifierVerdict, ScreenVerdict,
    CLASSIFIER_POLICY, REVIEW_QUESTION, SCREEN_QUESTION,
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
