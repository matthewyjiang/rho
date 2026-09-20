use rho_providers::{auth::login_dispatch::InteractiveLoginMode, model::catalog::LoginTarget};

use super::{login_flow_picker, parse_login_flow_value};

// Covers: dual-grant rows must encode auth plus mode and preselect preferred_mode
// Owner: login flow picker
#[test]
fn login_flow_picker_encodes_auth_and_preselects_preferred_mode() {
    let target = LoginTarget {
        provider: "openai-codex".into(),
        auth: "codex".into(),
        label: "Codex OAuth".into(),
    };
    for preferred in [InteractiveLoginMode::Browser, InteractiveLoginMode::Device] {
        let picker = login_flow_picker(&target, "Codex", preferred);
        let modes = picker
            .items
            .iter()
            .map(|item| parse_login_flow_value(&item.value).expect("flow value"))
            .collect::<Vec<_>>();
        pretty_assertions::assert_eq!(
            modes,
            vec![
                ("codex".into(), InteractiveLoginMode::Browser),
                ("codex".into(), InteractiveLoginMode::Device),
            ],
            "{preferred:?}"
        );
        let selected = parse_login_flow_value(&picker.items[picker.selected].value)
            .expect("selected flow value");
        pretty_assertions::assert_eq!(selected, ("codex".into(), preferred), "{preferred:?}");
    }
}

// Covers: flow values must not parse unless they name a known mode
// Owner: login flow picker
#[test]
fn parse_login_flow_value_rejects_unknown_suffixes() {
    let cases = [
        (
            "codex/browser",
            Some(("codex", InteractiveLoginMode::Browser)),
        ),
        (
            "codex/device",
            Some(("codex", InteractiveLoginMode::Device)),
        ),
        (
            "xai-oauth/browser",
            Some(("xai-oauth", InteractiveLoginMode::Browser)),
        ),
        ("codex", None),
        ("/device", None),
        ("codex/other", None),
        ("codex/", None),
    ];
    for (value, expected) in cases {
        pretty_assertions::assert_eq!(
            parse_login_flow_value(value),
            expected.map(|(auth, mode)| (auth.to_string(), mode)),
            "{value}"
        );
    }
}
