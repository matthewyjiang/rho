use std::sync::Arc;

use rho_providers::{
    auth::login_prompt::LoginPrompt,
    credentials::{CredentialError, CredentialResult, CredentialStore, MemoryCredentialStore},
    model::catalog::LoginTarget,
};

use crate::tui::{tests::test_app, ComposerMode, PendingLoginComposer, PendingLoginFinish};

/// A keyring that rejects every write, like a locked or missing backend.
#[derive(Debug)]
struct ReadOnlyCredentialStore;

impl CredentialStore for ReadOnlyCredentialStore {
    fn get_secret(&self, _account: &str) -> CredentialResult<Option<String>> {
        Ok(None)
    }

    fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
        Err(CredentialError::StoreUnavailable("locked".into()))
    }

    fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
        Ok(false)
    }
}

fn ollama_device_pending() -> PendingLoginComposer {
    PendingLoginComposer {
        target: LoginTarget {
            provider: "ollama-cloud".into(),
            auth: "ollama-cloud-device".into(),
            label: "Ollama Cloud device key".into(),
        },
        prompt: LoginPrompt::device_code(
            "https://ollama.com/connect",
            "WD4E-T6MC",
            None,
            "Approve this device on ollama.com.",
        ),
        finish: PendingLoginFinish::UserConfirms,
    }
}

// Covers: Enter on a user-confirmed login whose write fails must keep the
// pending login open (retry with Enter, Esc back to setup) instead of
// stranding the user on a bare composer.
// Owner: pending login composer
#[tokio::test]
async fn confirming_pending_login_closes_it_only_after_the_write_succeeds() {
    let stores: [(Arc<dyn CredentialStore>, bool); 2] = [
        (Arc::new(MemoryCredentialStore::default()), true),
        (Arc::new(ReadOnlyCredentialStore), false),
    ];
    for (store, recorded) in stores {
        let mut app = test_app();
        app.credential_store = store;
        app.input_ui
            .set_composer(ComposerMode::InteractivePending(ollama_device_pending()));

        let target = app.record_confirmed_login().await;

        pretty_assertions::assert_eq!(
            (
                target.map(|target| target.auth),
                matches!(app.input_ui.composer(), ComposerMode::InteractivePending(_)),
            ),
            (
                recorded.then(|| "ollama-cloud-device".to_string()),
                !recorded,
            ),
            "recorded={recorded}"
        );
    }
}
