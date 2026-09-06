use pretty_assertions::assert_eq;

use super::{HostChoice, HostInputRequest, HostInputResponse, HostQuestion, SelectionMode};

// Covers: hosts cannot attribute changed or unauthorized answers to a timeout.
// Owner: SDK host-input contract.
#[test]
fn timeout_provenance_requires_the_explicit_fallback() {
    use super::HostInputSource;
    let answers = HostInputResponse::new()
        .answer("mode", ["safe"])
        .answer("features", ["a"]);
    let timed = request()
        .with_timeout_fallback(answers.clone(), "Use safe mode")
        .unwrap();
    let fallback = timed.timeout_fallback().unwrap();
    assert_eq!(fallback.source(), HostInputSource::TimeoutFallback);
    timed.validate(fallback).unwrap();
    // Registering a fallback validates its answers, not its previous request's provenance.
    let rebuilt = request()
        .with_timeout_fallback(fallback.clone(), "Reuse the explicit fallback")
        .unwrap();
    assert_eq!(rebuilt.timeout_fallback(), Some(fallback));
    timed.validate(&answers).unwrap();
    assert!(request().validate(fallback).is_err());
    assert!(timed
        .validate(&fallback.clone().answer("mode", ["fast"]))
        .is_err());
    for invalid in [
        HostInputResponse::new(),
        answers.clone().answer("unknown", ["a"]),
    ] {
        assert!(request()
            .with_timeout_fallback(invalid, "Fallback")
            .is_err());
    }
    assert!(request().with_timeout_fallback(answers, " ").is_err());
}

fn request() -> HostInputRequest {
    HostInputRequest::questionnaire(
        "configure",
        vec![
            HostQuestion::new(
                "mode",
                "mode?",
                vec![
                    HostChoice::new("fast", "Fast"),
                    HostChoice::new("safe", "Safe"),
                ],
                SelectionMode::One,
            )
            .unwrap(),
            HostQuestion::new(
                "features",
                "features?",
                vec![HostChoice::new("a", "A"), HostChoice::new("b", "B")],
                SelectionMode::Many,
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn questionnaire_validates_complete_typed_answers() {
    let request = request();
    let response = HostInputResponse::new()
        .answer("mode", ["safe"])
        .answer("features", ["a", "b"]);

    request.validate(&response).unwrap();
    assert_eq!(response.answers()["mode"], ["safe"]);
}

#[test]
fn questionnaire_rejects_missing_unknown_duplicate_and_excess_answers() {
    let request = request();

    assert!(request
        .validate(&HostInputResponse::new().answer("mode", ["fast"]))
        .is_err());
    assert!(request
        .validate(
            &HostInputResponse::new()
                .answer("mode", ["unknown"])
                .answer("features", ["a"]),
        )
        .is_err());
    assert!(request
        .validate(
            &HostInputResponse::new()
                .answer("mode", ["fast", "safe"])
                .answer("features", ["a"]),
        )
        .is_err());
    assert!(request
        .validate(
            &HostInputResponse::new()
                .answer("mode", ["fast"])
                .answer("features", ["a", "a"]),
        )
        .is_err());
}

#[test]
fn questionnaire_accepts_omitted_optional_questions_and_rejects_unknown_ids() {
    let optional = HostQuestion::new(
        "details",
        "details?",
        vec![HostChoice::new("more", "More")],
        SelectionMode::Many,
    )
    .unwrap()
    .optional();
    let request = HostInputRequest::questionnaire(
        "optional",
        vec![
            HostQuestion::new(
                "mode",
                "mode?",
                vec![HostChoice::new("safe", "Safe")],
                SelectionMode::One,
            )
            .unwrap(),
            optional,
        ],
    )
    .unwrap();

    request
        .validate(&HostInputResponse::new().answer("mode", ["safe"]))
        .unwrap();
    assert!(request
        .validate(
            &HostInputResponse::new()
                .answer("mode", ["safe"])
                .answer("unknown", ["value"]),
        )
        .is_err());
}

#[test]
fn questionnaire_requires_unique_question_ids() {
    let question = HostQuestion::new(
        "same",
        "question",
        vec![HostChoice::new("yes", "Yes")],
        SelectionMode::One,
    )
    .unwrap();

    assert!(
        HostInputRequest::questionnaire("duplicate", vec![question.clone(), question]).is_err()
    );
}
