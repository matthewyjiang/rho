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
