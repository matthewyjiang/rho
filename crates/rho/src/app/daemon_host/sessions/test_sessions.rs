//! Sessions loaded into a [`Sessions`] as session tests use them.

use std::{path::PathBuf, sync::Arc};

use tempfile::TempDir;

use super::{open::OpenedSession, LoadedSession, Sessions};
use crate::{
    app::{
        daemon_host::jobs::{JobId, JobRecord, TurnRecord},
        session_runtime::{
            unattended_runtime_on, unsaved_unattended_runtime_on, HangUntilCancel, QuestionSupport,
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
    /// Saved, with its turn under way.
    Busy,
    /// Never saved, as a new session before its first prompt.
    Unsaved,
}

/// A session loaded in `sessions` as `load` says, on a new job, with a
/// provider that never answers, so nothing but the test ends its turns.
pub(super) async fn load(sessions: &Sessions, root: &TempDir, load: Load) -> Arc<LoadedSession> {
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
        Load::Unsaved => unsaved_unattended_runtime_on(root.path(), &config, sdk, provider).await,
    };
    let id = runtime.session_id().clone();
    let record = JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), id.clone());
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
        Load::Saved | Load::Unsaved => {}
    }
    let session = Arc::new(LoadedSession::new(
        &sessions.jobs.get(&job).unwrap(),
        Arc::clone(&sessions.jobs),
        OpenedSession {
            runtime,
            config,
            global: Arc::new(Config::default()),
            environment: SessionEnvironment::default(),
        },
        /*auths*/ &[],
        &sessions.wakes,
        &sessions.settings,
    ));
    sessions.loaded().insert(id, Arc::clone(&session));
    session
}
