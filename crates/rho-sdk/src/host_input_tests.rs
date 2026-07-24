use pretty_assertions::assert_eq;

use super::{HostChoice, HostInputRequest, HostInputResponse, HostQuestion, SelectionMode};

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
fn host_choice_exposes_optional_description() {
    let plain = HostChoice::new("fast", "Fast");
    let detailed = HostChoice::new("safe", "Safe").description("Run every check");

    assert_eq!(plain.description_text(), None);
    assert_eq!(detailed.description_text(), Some("Run every check"));
}

#[test]
fn host_question_default_selection_defaults_to_selected() {
    use super::DefaultSelection;

    let selected = HostQuestion::new(
        "mode",
        "mode?",
        vec![
            HostChoice::new("fast", "Fast"),
            HostChoice::new("safe", "Safe"),
        ],
        SelectionMode::One,
    )
    .unwrap()
    .default_value(serde_json::json!("safe"));
    let focused = selected
        .clone()
        .default_selection(DefaultSelection::Focused);

    assert_eq!(
        selected.default_selection_mode(),
        DefaultSelection::Selected
    );
    assert_eq!(focused.default_selection_mode(), DefaultSelection::Focused);
    assert_eq!(
        focused.default_value_ref(),
        Some(&serde_json::json!("safe"))
    );
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
