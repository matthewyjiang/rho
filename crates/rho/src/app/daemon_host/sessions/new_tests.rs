use std::{path::PathBuf, sync::Arc};

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, SessionChange, SessionConfigureParams};
use tempfile::TempDir;

use crate::{
    app::{
        daemon_host::{
            jobs::{JobRecord, JobRegistry, JobStore},
            sessions::{
                configure::set_up_started_session,
                options::started_session_defaults,
                test_sessions::{load, Load},
                Sessions,
            },
            wire::session_to_wire,
        },
        pull_requests::ScriptedForge,
        session_runtime::StartedSessionDefaults,
    },
    config::Config,
};

/// A daemon's sessions with nothing loaded, recording jobs under `home`.
fn new_sessions(home: &TempDir) -> Arc<Sessions> {
    Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
        ScriptedForge::new(),
    )
}

// Covers: a session started under a lead takes the choice of the nearest
// session a person opened: the lead's own when the lead starts it, and the
// lead's too when a subagent of the lead does, naming its own session,
// which the daemon never loads, as the parent. Otherwise the lead's model
// could start a looser session through a subagent.
// Owner: daemon session new.
#[tokio::test]
async fn a_session_a_subagent_starts_takes_its_leads_choice() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = new_sessions(&home);
    let lead = load(&sessions, &root, Load::Saved).await;
    sessions
        .configure(SessionConfigureParams {
            session: session_to_wire(lead.id.clone()),
            change: SessionChange::StartedSessionPermissionMode {
                mode: Some(wire::PermissionMode::Plan),
            },
        })
        .await
        .unwrap();
    // A subagent run of the lead's turn, under way, writing its own session.
    let subagent = rho_sdk::SessionId::new();
    sessions
        .jobs
        .register(JobRecord::new_subagent(
            lead.job.clone(),
            PathBuf::from("/work"),
            Some(subagent.clone()),
            "run".into(),
        ))
        .unwrap();

    let mut chosen = Vec::new();
    for parent in [&lead.id, &subagent] {
        let writer = sessions.job_writing(parent);
        let choosing = sessions.choosing_session(parent, writer.as_ref()).unwrap();
        let choice = sessions.chosen_by(&choosing).await.unwrap();
        chosen.push((choosing, choice));
    }

    let leads = (
        lead.id.clone(),
        wire::StartedSessionDefaults {
            permission_mode: Some(wire::PermissionMode::Plan),
            model: None,
        },
    );
    assert_eq!(chosen, vec![leads.clone(), leads]);
}

// Covers: a permission mode a newer rho saved for the sessions a session
// starts is kept as it was read, so a save does not erase it, and a session
// started under it fails to open rather than run in the config's mode,
// which may be looser.
// Owner: daemon session new.
#[tokio::test]
async fn a_session_started_under_an_unknown_saved_mode_fails_to_open() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = new_sessions(&home);
    let saved = r#"{"permission_mode":"paranoid"}"#;
    let read: StartedSessionDefaults = serde_json::from_str(saved).unwrap();
    let started = load(&sessions, &root, Load::Unsaved).await;
    let mut runtime = started.runtime.try_lock().unwrap();

    let opened = set_up_started_session(
        runtime.as_mut().unwrap(),
        &Config::default(),
        &rho_sdk::SessionId::new(),
        started_session_defaults(&read),
    )
    .await
    .map_err(|error| error.code);

    assert_eq!(
        (serde_json::to_string(&read).unwrap(), opened),
        (
            saved.to_owned(),
            Err(wire::ErrorCode::SessionOpenFailed.code())
        )
    );
}
