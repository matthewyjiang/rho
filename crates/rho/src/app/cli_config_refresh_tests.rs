use std::sync::Mutex;

use rho_providers::{
    credentials::{CredentialError, CredentialResult, CredentialStore},
    model::provider_models::set_provider_models_cache_dir_for_tests,
    provider::OPENROUTER_OAUTH_KEY_ACCOUNT,
};

use crate::{cli::Cli, config::Config};

use super::refresh_model_cache;

#[derive(Default)]
struct RecordingStore {
    requested_accounts: Mutex<Vec<String>>,
}

impl CredentialStore for RecordingStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.requested_accounts.lock().unwrap().push(account.into());
        Err(CredentialError::StoreUnavailable(account.into()))
    }

    fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
        unreachable!("refresh must not write credentials")
    }

    fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
        unreachable!("refresh must not delete credentials")
    }
}

#[tokio::test]
async fn cold_cache_refresh_uses_the_cli_auth_profile_credentials() {
    let cache = tempfile::tempdir().unwrap();
    set_provider_models_cache_dir_for_tests(Some(cache.path().into()));
    let store = RecordingStore::default();
    let cli = Cli {
        provider: Some("openrouter".into()),
        model: None,
        config: None,
        auth: Some("openrouter-oauth".into()),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: false,
        agent: None,
        reasoning: None,
        resume: None,
        command: None,
    };

    let error = match refresh_model_cache(&cli, &Config::default(), &store).await {
        Ok(_) => panic!("cold-cache refresh unexpectedly succeeded"),
        Err(error) => error,
    };
    set_provider_models_cache_dir_for_tests(None);

    assert!(error.to_string().contains(OPENROUTER_OAUTH_KEY_ACCOUNT));
    assert_eq!(
        *store.requested_accounts.lock().unwrap(),
        vec![OPENROUTER_OAUTH_KEY_ACCOUNT]
    );
}
