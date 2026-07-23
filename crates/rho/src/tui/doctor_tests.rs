use super::*;
use pretty_assertions::assert_eq;
use rho_providers::credentials::MemoryCredentialStore;

#[test]
fn directory_probe_rejects_regular_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions");
    fs::write(&path, "not a directory").unwrap();

    assert!(!probe_writable(&path, PathKind::Directory));
}

#[test]
fn ollama_diagnostics_show_no_auth_and_each_connection_state() {
    let cases = [
        (
            ProviderModelHealth::ReachableWithModels { model_count: 2 },
            "reachable",
            PickerBadgeTone::Healthy,
            "returned 2 installed models",
        ),
        (
            ProviderModelHealth::ReachableWithoutModels,
            "no models",
            PickerBadgeTone::Warning,
            "reachable but has no installed models",
        ),
        (
            ProviderModelHealth::Unreachable {
                error: "connection refused".into(),
            },
            "unreachable",
            PickerBadgeTone::Warning,
            "connection refused",
        ),
        (
            ProviderModelHealth::InvalidResponse {
                error: "HTTP 500".into(),
            },
            "invalid response",
            PickerBadgeTone::Warning,
            "HTTP 500",
        ),
    ];

    for (health, expected_status, expected_tone, expected_detail) in cases {
        let directory = tempfile::tempdir().unwrap();
        let store = MemoryCredentialStore::default();
        let health = [("ollama".to_string(), health)];
        let picker = picker(DoctorContext {
            provider: "ollama",
            model: "local-model",
            auth: "none",
            available_auths: &["none".into()],
            credential_store: &store,
            config_path: &directory.path().join("config.toml"),
            session_root: &directory.path().join("sessions"),
            herdr_enabled: false,
            herdr_socket_reachable: None,
            provider_health: &health,
        });
        assert_eq!(picker.layout, PickerLayout::Overlay);
        assert_eq!(picker.badge_placement, PickerBadgePlacement::Detail);
        let chrome = picker.overlay_chrome.as_ref().unwrap();
        assert_eq!(chrome.nav_label, " CHECKS");
        assert_eq!(chrome.detail_label.as_deref(), Some(" DETAILS"));
        assert_eq!(chrome.nav_keys_hint, "↑↓ checks");
        assert_eq!(picker.confirm_action_label(), "close");
        let labels = picker
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        for label in [
            "OpenRouter API key",
            "OpenRouter OAuth",
            "xAI API key",
            "xAI OAuth",
            "OpenRouter API key model cache",
            "OpenRouter OAuth model cache",
        ] {
            assert!(labels.contains(&label), "missing {label} in {labels:?}");
        }
        let sections = picker
            .items
            .iter()
            .filter_map(|item| item.section.as_deref())
            .fold(Vec::new(), |mut sections, section| {
                if sections.last().copied() != Some(section) {
                    sections.push(section);
                }
                sections
            });
        assert_eq!(sections, ["AUTHENTICATION", "CACHE", "MISC"]);

        let auth = picker
            .items
            .iter()
            .find(|item| item.label == "Ollama authentication")
            .unwrap();
        let connection = picker
            .items
            .iter()
            .find(|item| item.label == "Ollama connection")
            .unwrap();

        assert_eq!(
            auth.badge.as_ref().unwrap().text,
            "no authentication required"
        );
        assert_eq!(auth.badge.as_ref().unwrap().tone, PickerBadgeTone::Healthy);
        assert_eq!(connection.badge.as_ref().unwrap().text, expected_status);
        assert_eq!(connection.badge.as_ref().unwrap().tone, expected_tone);
        assert!(
            connection
                .detail
                .as_deref()
                .unwrap()
                .contains(expected_detail),
            "{:?}",
            connection.detail
        );
    }
}
