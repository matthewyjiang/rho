//! Signing in on a machine with no session D-Bus, as an SSH login's
//! (#1662): the daemon a client starts there cannot reach the OS store,
//! and the way out is the file store, chosen at `rho login`'s prompt or in
//! config from outside the daemon. Either way the next session signs in
//! with what is kept there, against a provider that checks every key over
//! real HTTP.
//!
//! The sandbox's empty `XDG_RUNTIME_DIR` is the missing session bus, so
//! these run on Linux only: no other OS store needs a bus.
#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::SessionNew, Client, ClientEnvironment, CredentialStoreKind, CredentialStoreStatus,
    ErrorCode, JobState, SessionId, SessionNewParams, SessionSetup, StoreSelectedBy,
};
use rho_providers::credentials::CredentialStore;
use rho_tui_pty::{Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};

use super::auth::{rpc_error, store_status, Fixture, FIRST_KEY, KEY_ACCOUNT};

/// The variable that chooses a rho process's credential store, which the
/// sandbox sets for every rho it starts.
const STORE_VARIABLE: &str = "RHO_CREDENTIAL_STORE";

/// The environment of a shell on the headless machine: the sandbox's, with
/// nothing choosing the credential store and no browser to open.
fn headless_environment(fixture: &Fixture) -> ClientEnvironment {
    fixture
        .sandbox
        .client_environment()
        .into_iter()
        .filter(|(name, _)| name != STORE_VARIABLE)
        .chain([(
            "SSH_CONNECTION".to_owned(),
            "127.0.0.1 1 127.0.0.1 2".to_owned(),
        )])
        .collect()
}

/// Opens a session for a client in the headless shell; the error code of a
/// refused one.
async fn open_session(fixture: &Fixture, client: &Client) -> Result<SessionId, Option<ErrorCode>> {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: fixture.sandbox.workspace.clone(),
            environment: headless_environment(fixture),
            agent: None,
            setup: SessionSetup::default(),
            parent: None,
        })
        .await
        .map(|opened| opened.session)
        .map_err(|error| rpc_error(error).error_code())
}

/// The file store, chosen in the daemon's config.
fn file_store_from_config() -> CredentialStoreStatus {
    CredentialStoreStatus {
        store: CredentialStoreKind::File,
        selected_by: StoreSelectedBy::Config,
        unreachable: None,
    }
}

// Covers: `rho login` at a terminal with no session D-Bus and no browser
// starts the daemon, which cannot reach the OS store and so opens no
// session; Enter at the store prompt takes the file store, the only one
// offered as available, the daemon keeps the key there, and the next
// session's turn presents that key to the provider (#1662 acceptance 1).
// Owner: rho login CLI against the daemon it starts (PTY, real terminal),
// and that daemon's sessions over its socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rho_login_from_a_headless_shell_signs_in_the_daemons_next_session() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = Fixture::without_daemon(&valid);
    let plan = RhoLaunchPlan {
        binary: env!("CARGO_BIN_EXE_rho").into(),
        size: PtySize {
            rows: 28,
            cols: 200,
        },
        args: vec!["login".into(), "mock".into()],
        env: headless_environment(&fixture).into_iter().collect(),
        cwd: fixture.sandbox.workspace.clone(),
    };
    let mut login = PtyHarness::spawn_named(&plan, "rho_login_headless_session").unwrap();
    // The harness waits on the terminal, which blocks this worker.
    let mut at_terminal = |step: &mut dyn FnMut(&mut PtyHarness)| {
        tokio::task::block_in_place(|| step(&mut login));
    };

    at_terminal(&mut |login| {
        login
            .wait_for_text("input hidden", WaitTimeout::secs(20, "key prompt"))
            .unwrap();
        login.type_text(FIRST_KEY).unwrap();
        login.inject_key(&Key::Enter).unwrap();
        // The file store is what Enter takes only when the OS store is not
        // offered.
        login
            .wait_for_text("(default 2)", WaitTimeout::secs(20, "store prompt"))
            .unwrap();
    });
    // `rho login` asked the daemon it started about its store, so it answers.
    let client = fixture.sandbox.connect().await;
    let before = (
        open_session(&fixture, &client).await.err(),
        fixture.saved_key(),
    );
    let mut exit = None;
    at_terminal(&mut |login| {
        login.inject_key(&Key::Enter).unwrap();
        exit = Some(
            login
                .wait_for_exit(WaitTimeout::secs(30, "rho login exits"))
                .unwrap(),
        );
    });
    let session = open_session(&fixture, &client).await;
    let turn = match &session {
        Ok(session) => Some(
            fixture
                .prompt_from(&client, session, headless_environment(&fixture))
                .await,
        ),
        Err(_) => None,
    };

    assert_eq!(
        (
            before,
            exit,
            store_status(&client).await,
            fixture.saved_key(),
            session.map(drop),
            turn,
        ),
        (
            (Some(Some(ErrorCode::SignInRequired)), None),
            Some(0),
            file_store_from_config(),
            Some(FIRST_KEY.into()),
            Ok(()),
            Some(Ok(JobState::Completed)),
        ),
        "log:\n{}",
        fixture.sandbox.isolated.daemon_log()
    );
}

// Covers: a daemon that cannot reach the OS store follows a store chosen
// from outside it, as `rho credential-store set file` does: the session
// it opens next reads the credentials kept there, where before it opened
// none, with no credential write through the daemon in between.
// Owner: daemon session open (the credential store its config selects).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_chosen_outside_the_daemon_is_the_one_its_next_session_reads() {
    let valid = Arc::new(Mutex::new(FIRST_KEY.to_owned()));
    let fixture = Fixture::start_with(&valid, |command| {
        command.env_remove(STORE_VARIABLE);
    })
    .await;
    // A key from when this home kept its credentials in the file store.
    fixture.store.set_secret(KEY_ACCOUNT, FIRST_KEY).unwrap();
    let client = fixture.sandbox.connect().await;
    let before = open_session(&fixture, &client).await.err();

    let chosen = fixture
        .sandbox
        .rho("fixed")
        .env_remove(STORE_VARIABLE)
        .args(["credential-store", "set", "file"])
        .status()
        .unwrap();
    let session = open_session(&fixture, &client)
        .await
        .expect("the daemon reads the file store");

    assert_eq!(
        (
            before,
            chosen.success(),
            store_status(&client).await,
            fixture
                .prompt_from(&client, &session, headless_environment(&fixture))
                .await,
        ),
        (
            Some(Some(ErrorCode::SignInRequired)),
            true,
            file_store_from_config(),
            Ok(JobState::Completed),
        ),
        "log:\n{}",
        fixture.sandbox.isolated.daemon_log()
    );
}
