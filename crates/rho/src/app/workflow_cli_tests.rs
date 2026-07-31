use super::*;

// Covers: malformed or duplicate --input values could produce an ambiguous frozen plan.
// Owner: workflow CLI input parser.
#[test]
fn parses_inputs_as_typed_values_and_rejects_ambiguous_entries() {
    let parsed = parse_inputs(&[
        "target=\"src\"".to_owned(),
        "attempts=3".to_owned(),
        "enabled=true".to_owned(),
    ])
    .unwrap();
    assert_eq!(
        parsed,
        BTreeMap::from([
            (
                InputName::new("attempts").unwrap(),
                WorkflowValue::Integer(3),
            ),
            (
                InputName::new("enabled").unwrap(),
                WorkflowValue::Bool(true),
            ),
            (
                InputName::new("target").unwrap(),
                WorkflowValue::String("src".to_owned()),
            ),
        ])
    );

    for values in [
        vec!["missing-separator".to_owned()],
        vec!["target=not-json".to_owned()],
        vec!["target=1".to_owned(), "target=2".to_owned()],
    ] {
        assert!(parse_inputs(&values).is_err());
    }
}

// Covers: a redirected run could start without consent, or --yes could still prompt.
// Owner: workflow CLI confirmation policy.
#[test]
fn confirmation_policy_requires_yes_only_when_not_interactive() {
    assert_eq!(
        confirmation_requirement(true, false),
        ConfirmationRequirement::Confirmed
    );
    assert_eq!(
        confirmation_requirement(true, true),
        ConfirmationRequirement::Confirmed
    );
    assert_eq!(
        confirmation_requirement(false, true),
        ConfirmationRequirement::Prompt
    );
    assert_eq!(
        confirmation_requirement(false, false),
        ConfirmationRequirement::FlagRequired
    );
}
