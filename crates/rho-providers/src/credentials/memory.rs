//! In-memory credential store for tests and debug builds.

use std::{collections::HashMap, sync::Mutex};

use super::{CredentialResult, CredentialStore};

#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        Ok(self.secrets.lock().unwrap().get(account).cloned())
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.secrets
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        Ok(self.secrets.lock().unwrap().remove(account).is_some())
    }
}
