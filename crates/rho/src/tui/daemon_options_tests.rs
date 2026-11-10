use pretty_assertions::assert_eq;
use rho_protocol::{ModelChoice, ModelOptions, ModelRef};

use super::{model_choice, next_choice};

// Covers: a bare id shared by two providers must not silently select the
// wrong provider; fully qualified choices work without any local catalog.
// Owner: pure daemon option selection
#[test]
fn resolve_only_unambiguous_advertised_models() {
    let first = ModelRef {
        provider: "private-host".into(),
        model: "shared".into(),
    };
    let second = ModelRef {
        provider: "other-host".into(),
        model: "shared".into(),
    };
    let options = ModelOptions {
        auth: None,
        current: first.clone(),
        available: vec![
            ModelChoice {
                model: first.clone(),
                label: "Private model".into(),
            },
            ModelChoice {
                model: second.clone(),
                label: "Other model".into(),
            },
        ],
    };
    for (reference, expected) in [
        ("private-host/shared", Some(first.clone())),
        ("Private model", Some(first)),
        ("other-host/shared", Some(second)),
        ("shared", None),
        ("openai/not-advertised", None),
    ] {
        assert_eq!(
            model_choice(&options, reference).ok(),
            expected,
            "{reference}"
        );
    }
}

// Covers: cycling uses the server's offered order, wraps, and does not invent
// a choice when the current value is unknown to this client.
// Owner: pure daemon option selection
#[test]
fn cycle_advertised_choices_without_catalog_fallbacks() {
    use rho_protocol::ReasoningLevel::{High, Low, Max, Unknown};
    for (current, available, expected) in [
        (Low, vec![Low, Max, High], Some(Max)),
        (High, vec![Low, Max, High], Some(Low)),
        (High, vec![High], Some(High)),
        (Unknown, vec![Low, High], None),
        (Low, vec![], None),
    ] {
        assert_eq!(
            next_choice(current, &available),
            expected,
            "{current:?}, {available:?}"
        );
    }
}
