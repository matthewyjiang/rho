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
        let choosing = sessions
            .choosing_session(parent, writer.as_ref())
            .await
            .unwrap();
        let choice = sessions.chosen_by(&choosing).await.unwrap();
        chosen.push((choosing.id, choice));
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

// Covers: a choice for the sessions a session starts that this rho cannot
// use, a mode a newer rho named, a field it added, or a record it cannot
// read at all, fails every session started under it rather than open it on
// the config's, which may be looser, and a save keeps the record byte for
// byte, model included, so a rho that reads it still finds it.
// Owner: daemon session new.
#[tokio::test]
async fn a_saved_choice_this_rho_cannot_use_refuses_new_and_survives_a_save() {
    let saved = [
        r#"{"permission_mode":"paranoid"}"#,
        r#"{"permission_mode":{"name":"plan"},"model":{"provider":"openai","model":"gpt-5"}}"#,
        r#"{"permission_mode":"plan","sandbox":"strict"}"#,
        "[]",
    ];
    let mut outcomes = Vec::new();
    for record in saved {
        let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let sessions = new_sessions(&home);
        let choosing = load(&sessions, &root, Load::Saved).await;
        let started = load(&sessions, &root, Load::Unsaved).await;
        let mut choosing_runtime = choosing.runtime.try_lock().unwrap();
        let choosing_runtime = choosing_runtime.as_mut().unwrap();
        let mut started_runtime = started.runtime.try_lock().unwrap();
        choosing_runtime.reopen_with_started_sessions_record_for_tests(record);

        let opened = set_up_started_session(
            started_runtime.as_mut().unwrap(),
            &Config::default(),
            &choosing.id,
            started_session_defaults(choosing_runtime.started_session_defaults()),
        )
        .await
        .map_err(|error| error.code);
        choosing_runtime.save_state().unwrap();
        outcomes.push((
            opened,
            choosing_runtime.saved_started_sessions_record_for_tests(),
        ));
    }

    let refused = Err(wire::ErrorCode::SessionOpenFailed.code());
    assert_eq!(
        outcomes,
        saved
            .map(|record| (refused, Some(record.to_owned())))
            .to_vec()
    );
}
