use std::{collections::BTreeMap, sync::Arc};

use pretty_assertions::assert_eq;
use rho_providers::credentials::CredentialStore;

use super::ClientCredentialStore;

fn client(variables: &[(&str, &str)]) -> ClientCredentialStore {
    ClientCredentialStore::new(Arc::new(
        variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    ))
}

// Covers: a daemon client's credentials answer environment lookups from the
// variables that client sent and from nothing else, so a session never
// reads a key from the daemon's own environment: a client that sets a key
// gets its own, and one that sets none gets none, even for a variable this
// process does have (#1779).
// Owner: unit (daemon client credentials)
#[test]
fn a_clients_credentials_read_only_that_clients_environment() {
    // Cargo sets it for every test process, so the process has it.
    let in_this_process = "CARGO_MANIFEST_DIR";
    assert!(std::env::var_os(in_this_process).is_some());
    let cases = [
        ("no variables", client(&[]), [None, None]),
        (
            "its own key",
            client(&[("OPENAI_API_KEY", "the client's")]),
            [Some("the client's"), None],
        ),
        (
            "a variable this process has too",
            client(&[(in_this_process, "the client's")]),
            [None, Some("the client's")],
        ),
    ];
    for (name, credentials, expected) in cases {
        for store in [credentials.clone(), credentials.untracked()] {
            assert_eq!(
                ["OPENAI_API_KEY", in_this_process].map(|name| store.environment_variable(name)),
                expected.map(|value| value.map(str::to_owned)),
                "{name}"
            );
        }
    }
}

// Covers: a client's `RHO_CREDENTIAL_STORE` chooses the store its
// credentials are kept in, and a value that names no store refuses every
// access instead of falling back to the daemon's own store.
// Owner: unit (daemon client credentials)
#[test]
fn an_unknown_store_in_a_clients_environment_refuses_every_access() {
    let credentials = client(&[("RHO_CREDENTIAL_STORE", "plaintext-maybe")]);

    assert_eq!(
        (
            credentials.get_secret("account").is_err(),
            credentials.get_secret_fresh("account").is_err(),
            credentials.set_secret("account", "secret").is_err(),
            credentials.delete_secret("account").is_err(),
        ),
        (true, true, true, true)
    );
}
