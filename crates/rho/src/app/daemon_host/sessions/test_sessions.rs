//! Sessions loaded into a [`Sessions`] as session tests use them.

use std::{path::PathBuf, sync::Arc};

use tempfile::TempDir;

use super::{background::SessionBackground, open::OpenedSession, LoadedSession, Sessions};
use crate::{
    app::{
        daemon_host::jobs::{JobId, JobRecord, TurnRecord},
        session_runtime::{
            idle_woken_runtime_on, unattended_runtime_on, unsaved_unattended_runtime_on,
            HangUntilCancel, QuestionSupport,
        },
    },
    config::Config,
    session_environment::SessionEnvironment,
};

/// How a test session is loaded.
#[derive(Clone, Copy)]
pub(super) enum Load {
    /// Saved, with no turn under way.
    Saved,
    /// As [`Self::Saved`], its client having asked the daemon to start a
    /// turn when its background work finishes idle (`idle_wake: turn`), so
    /// its tools hold its pull request inbox, as the daemon opens it.
    Waking,
    /// Saved, with its turn under way.
    Busy,
    /// Never saved, as a new session before its first prompt.
    Unsaved,
}

/// A session loaded in `sessions` as `load` says, on a new job, with a
/// provider that never answers, so nothing but the test ends its turns.
pub(super) async fn load(sessions: &Sessions, root: &TempDir, load: Load) -> Arc<LoadedSession> {
    insert(sessions, opened(sessions, root, load).await)
}

/// A saved session loaded in `sessions` as [`load`] loads it, whose
/// background work is what `background` makes of its id rather than its
/// runtime's tools'.
pub(super) async fn load_waiting_on(
    sessions: &Sessions,
    root: &TempDir,
    background: impl FnOnce(&str) -> SessionBackground,
) -> Arc<LoadedSession> {
    let session = opened(sessions, root, Load::Saved).await;
    let background = background(session.id.as_str());
    insert(
        sessions,
        LoadedSession {
            background,
            ..session
        },
    )
}

fn insert(sessions: &Sessions, session: LoadedSession) -> Arc<LoadedSession> {
    let session = Arc::new(session);
    sessions
        .loaded()
        .insert(session.id.clone(), Arc::clone(&session));
    session
}

async fn opened(sessions: &Sessions, root: &TempDir, load: Load) -> LoadedSession {
    let config = Config::default();
    let provider: Arc<dyn rho_sdk::provider::ModelProvider> = Arc::new(HangUntilCancel);
    let sdk = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let runtime = match load {
        Load::Saved | Load::Busy => {
            let questions = QuestionSupport::Unavailable;
            unattended_runtime_on(root.path(), &config, questions, sdk, provider).await
        }
        Load::Waking => {
            let inbox =
                |id: &rho_sdk::SessionId| Some(sessions.pull_requests.inbox(id, root.path()));
            idle_woken_runtime_on(root.path(), &config, sdk, provider, inbox).await
        }
        Load::Unsaved => unsaved_unattended_runtime_on(root.path(), &config, sdk, provider).await,
    };
    let id = runtime.session_id().clone();
    let record = JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), id);
    let job = record.id.clone();
    sessions.jobs.register(record).unwrap();
    match load {
        Load::Busy => {
            sessions
                .jobs
                .start_turn(
                    &job,
                    TurnRecord::starting(/*key*/ None),
                    crate::caller_context::CallerContext::ROOT,
                )
                .unwrap();
        }
        Load::Saved | Load::Waking | Load::Unsaved => {}
    }
    LoadedSession::new(
        &sessions.jobs.get(&job).unwrap(),
        Arc::clone(&sessions.jobs),
        OpenedSession {
            runtime,
            config,
            global: Arc::new(Config::default()),
            environment: SessionEnvironment::default(),
            setup: rho_protocol::SessionSetup::default(),
        },
        /*auths*/ &[],
        &sessions.wakes,
        &sessions.settings,
    )
}
