use std::{collections::BTreeMap, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::{model::ModelIdentity, provider::ScriptedProvider};

use super::{CredentialFailure, SessionCredentials};
use crate::{
    app::session_runtime::{
        test_support::{built_on, test_build},
        SessionRuntime, SessionStorage,
    },
    config::Config,
    credential_store::AppCredentialStore,
    session::Session as StoredSession,
    session_environment::{Ambient, SessionEnvironment},
};

// Covers: a renewal that finds nothing to build with, as the first prompt
// after a logout does, has the next prompt reread logins other processes
// saved. Its own read cached the miss in the OS store, and a login from
// another process moves no generation here, so without the reread the
// session stays signed out of a store that holds a credential (#1662).
// Owner: session runtime credential renewal. The OS store's cache needs a
// keyring, so this checks the state that orders the reread.
#[tokio::test]
async fn a_renewal_that_finds_no_credentials_rereads_other_processes_logins_next() {
    let root = tempfile::tempdir().unwrap();
    // A provider nothing in the test home signs in to, for a client whose
    // environment holds no key either and names its own store, whatever
    // store another test left this process with.
    let config = Config {
        provider: "minimax".into(),
        model: "unsigned".into(),
        auth: "minimax-api-key".into(),
        ..Config::default()
    };
    let mut build = test_build(&config, root.path());
    build.host.environment =
        SessionEnvironment::new(Ambient::Client(Arc::new(BTreeMap::from([(
            "RHO_CREDENTIAL_STORE".to_owned(),
            "file".to_owned(),
        )]))));
    let stored = StoredSession::create_in_root(root.path(), root.path()).unwrap();
    let id = rho_sdk::SessionId::from_string(stored.id()).unwrap();
    let provider = Arc::new(ScriptedProvider::new(
        ModelIdentity::new(&config.provider, "scripted", &config.model),
        Vec::new(),
    ));
    let built = built_on(&config, id, provider).await;
    let mut runtime =
        SessionRuntime::for_tests_with(build, built, SessionStorage::saved(stored), &config);
    // The provider read its credentials before a logout moved the
    // generation.
    runtime.credentials = SessionCredentials::Read(AppCredentialStore::generation());
    AppCredentialStore::note_changed();

    let failure = runtime
        .begin_prompt(&config)
        .await
        .err()
        .and_then(|error| CredentialFailure::find(&error));

    assert_eq!(
        (failure.is_some(), runtime.credentials),
        (true, SessionCredentials::Refused)
    );
}
