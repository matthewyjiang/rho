use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialResult, CredentialStoreBackend};
use tempfile::TempDir;

use super::*;

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, String>>);

impl CredentialStore for MemoryStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.0
            .lock()
            .unwrap()
            .insert(account.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        Ok(self.0.lock().unwrap().remove(account).is_some())
    }
}

struct StaleCache {
    cached: MemoryStore,
    current: MemoryStore,
}

impl CredentialStore for StaleCache {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.cached.get_secret(account)
    }

    fn get_secret_fresh(&self, account: &str) -> CredentialResult<Option<String>> {
        self.current.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.current.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.current.delete_secret(account)
    }
}

// Covers: the actual application wrapper must not turn fresh authorization
// reads into cached reads: neither an old hit nor an old miss may hide the
// backing token set after another process revokes or creates a token.
// Owner: MCP credential-store boundary through the selected-store decorator.
#[test]
fn token_reads_bypass_cached_hits_and_misses_through_the_application_wrapper() {
    let root = TempDir::new().unwrap();
    let mut tokens = TokenSet::default();
    let secret = tokens
        .rotate(
            "remote",
            Rotation {
                access: None,
                workspaces: vec![root.path().to_owned()],
            },
        )
        .unwrap();
    let observed: Vec<_> = [
        ("revoked", tokens.clone(), TokenSet::default()),
        ("cached-miss", TokenSet::default(), tokens),
    ]
    .into_iter()
    .map(|(case, cached, current)| {
        let store = StaleCache {
            cached: MemoryStore::default(),
            current: MemoryStore::default(),
        };
        cached.save(&store.cached).unwrap();
        current.save(&store.current).unwrap();
        let store =
            crate::credential_store::notifying_store(Arc::new(store), CredentialStoreBackend::Os);
        (
            case,
            TokenSet::load(store.as_ref())
                .unwrap()
                .authenticate(&secret)
                .is_some(),
        )
    })
    .collect();

    assert_eq!(observed, vec![("revoked", false), ("cached-miss", true)]);
}

// Covers: a new token defaults to read access over the canonical
// directories it was given, its secret authenticates as its grant after a
// save and load, a rotation keeps the token's grant and retires the old
// secret, and a revoke retires the token.
// Owner: MCP server tokens.
#[test]
fn tokens_rotate_authenticate_and_revoke() {
    let store = MemoryStore::default();
    let root = TempDir::new().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let mut tokens = TokenSet::default();
    let first = tokens
        .rotate(
            "remote",
            Rotation {
                access: None,
                workspaces: vec![root.path().join(".")],
            },
        )
        .unwrap();
    tokens.save(&store).unwrap();
    let mut loaded = TokenSet::load(&store).unwrap();
    let granted = loaded.authenticate(&first).map(TokenRecord::grant);
    let second = loaded
        .rotate(
            "remote",
            Rotation {
                access: Some(Access::Write),
                workspaces: Vec::new(),
            },
        )
        .unwrap();
    let rotated = (
        loaded.authenticate(&first).is_some(),
        loaded.authenticate(&second).map(TokenRecord::grant),
    );
    let revoked = (loaded.revoke("remote"), loaded.revoke("remote"));

    assert_eq!(
        (
            granted,
            rotated,
            revoked,
            loaded.authenticate(&second).is_some()
        ),
        (
            Some(Grant {
                holder: Holder::Token("remote".into()),
                scope: Scope {
                    access: Access::Read,
                    workspaces: Workspaces::Only(vec![workspace.clone()]),
                },
            }),
            (
                false,
                Some(Grant {
                    holder: Holder::Token("remote".into()),
                    scope: Scope {
                        access: Access::Write,
                        workspaces: Workspaces::Only(vec![workspace]),
                    },
                })
            ),
            (true, false),
            false,
        )
    );
    assert!(first.starts_with(SECRET_PREFIX) && first != second);
}

// Covers: a new token without a workspace, and a name refusals could not
// show plainly, are refused before anything is saved.
// Owner: MCP server tokens.
#[test]
fn a_new_token_needs_a_workspace_and_a_plain_name() {
    let root = TempDir::new().unwrap();
    let mut tokens = TokenSet::default();
    let rotation = |workspaces| Rotation {
        access: None,
        workspaces,
    };

    assert_eq!(
        [
            tokens
                .rotate("remote", rotation(Vec::new()))
                .map_err(|error| error.to_string()),
            tokens
                .rotate("has space", rotation(vec![root.path().to_path_buf()]))
                .map_err(|error| error.to_string()),
        ],
        [
            Err("a new token needs at least one --workspace: the directories whose sessions it may reach".into()),
            Err("token name \"has space\" must be 1 to 64 ASCII letters, digits, '-', or '_'".into()),
        ]
    );
    assert_eq!(tokens, TokenSet::default());
}
