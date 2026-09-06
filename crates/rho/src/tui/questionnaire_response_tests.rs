use super::*;
use pretty_assertions::assert_eq;

// Covers: display labels must not replace machine values in host responses.
// Owner: pure questionnaire response construction (not terminal rendering).
#[test]
fn submission_preserves_machine_values_and_displays_labels() {
    for (choices, selection, expected_values, expected_display) in [
        (
            vec![("rust", "Rust"), ("go", "Go")],
            SelectionMode::One,
            vec!["rust"],
            "Rust",
        ),
        (
            vec![("yes", "Yes"), ("no", "No")],
            SelectionMode::One,
            vec!["yes"],
            "Yes",
        ),
        (
            vec![("unit_tests", "Unit tests"), ("e2e", "End to end")],
            SelectionMode::Many,
            vec!["unit_tests", "e2e"],
            "Unit tests, End to end",
        ),
    ] {
        let question = HostQuestion::new(
            "answer",
            "Choose",
            choices
                .into_iter()
                .map(|(value, label)| HostChoice::new(value, label))
                .collect(),
            selection,
        )
        .unwrap();
        let request = HostInputRequest::questionnaire("Setup", vec![question]).unwrap();
        let (host, display) = submit(request.clone(), |composer| {
            composer.toggle_active_choice();
            if selection == SelectionMode::Many {
                composer.move_active_choice_next();
                composer.toggle_active_choice();
            }
        });
        assert_eq!(
            host,
            HostInputResponse::new().answer("answer", expected_values)
        );
        assert_eq!(display, expected_display);
        request.validate(&host).unwrap();
    }
}

#[test]
fn focused_default_requires_selection_and_returns_its_value() {
    let question = HostQuestion::new(
        "prompt",
        "Prompt mode?",
        vec![
            HostChoice::new("replace", "Replace"),
            HostChoice::new("extend", "Extend"),
        ],
        SelectionMode::One,
    )
    .unwrap()
    .default_value(serde_json::json!("extend"))
    .default_selection(DefaultSelection::Focused);
    let request = HostInputRequest::questionnaire("Prompt", vec![question]).unwrap();
    let (host, display) = submit(request.clone(), |composer| {
        assert!(composer.submit().is_err());
        composer.toggle_active_choice();
    });
    assert_eq!(host, HostInputResponse::new().answer("prompt", ["extend"]));
    assert_eq!(display, "Extend");
    request.validate(&host).unwrap();
}

// Covers: optional omission differs from an explicitly registered empty fallback.
// Owner: typed questionnaire reply channel; no SDK/JSON/SDK conversion is needed.
#[test]
fn optional_answers_preserve_user_omission_and_explicit_fallback() {
    let question = HostQuestion::new(
        "language",
        "Language?",
        vec![HostChoice::new("rust", "Rust")],
        SelectionMode::One,
    )
    .unwrap()
    .optional();
    let request = HostInputRequest::questionnaire("Optional", vec![question]).unwrap();
    let (host, _) = submit(request.clone(), |_| {});
    assert_eq!(host, HostInputResponse::new());
    request.validate(&host).unwrap();

    let fallback = HostInputResponse::new().answer("language", Vec::<String>::new());
    let request = request
        .with_timeout_fallback(fallback, "Leave language unset")
        .unwrap();
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer =
        QuestionnaireComposer::new(request.clone(), QuestionnaireResponseChannel::new(reply_tx));
    let now = std::time::Instant::now();
    composer.start_timeout(std::num::NonZeroU64::new(1), now);
    assert!(composer
        .submit_timeout_if_due(now + std::time::Duration::from_secs(1))
        .is_some());
    let QuestionnaireReply::Answer(host) = reply_rx.try_recv().unwrap() else {
        panic!("expected fallback response");
    };
    assert_eq!(Some(&host), request.timeout_fallback());
    request.validate(&host).unwrap();
}

fn submit(
    request: HostInputRequest,
    interact: impl FnOnce(&mut QuestionnaireComposer),
) -> (HostInputResponse, String) {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer =
        QuestionnaireComposer::new(request, QuestionnaireResponseChannel::new(reply_tx));
    interact(&mut composer);
    let submitted = composer.submit().unwrap();
    let QuestionnaireReply::Answer(response) = reply_rx.try_recv().unwrap() else {
        panic!("expected questionnaire answer");
    };
    (response, submitted.display)
}
