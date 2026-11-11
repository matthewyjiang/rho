use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_protocol::{
    self as wire, AnswerStop, ContentBlock, SessionChange, SessionConfigureParams, TurnOutcome,
};
use rho_sdk::model::Message;
use tempfile::TempDir;

use super::{
    opening_feed,
    test_sessions::{load, Load},
    Sessions,
};
use crate::{
    app::daemon_host::{
        jobs::{JobRegistry, JobStore, TurnEnd, TurnRecord},
        wire::session_to_wire,
    },
    caller_context::CallerContext,
    config::Config,
};

/// What a reopened session's replay shows of its conversation.
#[derive(Debug, PartialEq)]
enum Replayed {
    User(String),
    Answer(String),
}

// Covers: a session whose conversation moved back to an earlier node of its
// tree after a turn answered, then reopened from its job's record as after
// an unload, replays the restored conversation alone. The record's last
// answer is of a turn the conversation no longer holds, and a TUI appends a
// replayed answer the rows before it did not show (#1818).
// Owner: daemon session reopening.
#[tokio::test]
async fn a_reopened_session_replays_no_answer_its_restore_left_behind() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let sessions = Sessions::new(jobs, home.path());
    let session = load(&sessions, &root, Load::Saved).await;
    let first = {
        let mut runtime = session.runtime.lock().await;
        let runtime = runtime.as_mut().unwrap();
        for text in ["first", "second"] {
            runtime
                .append_context(text.into(), Message::user_text(text), [])
                .unwrap();
        }
        super::turn::save_transcript(&session, runtime);
        let rows = runtime.saved_history().unwrap().display_rows().unwrap();
        rows[0].origin.as_ref().unwrap().node.clone()
    };
    let turn = TurnRecord::starting(/*key*/ None);
    let id = turn.id.clone();
    sessions
        .jobs
        .start_turn(&session.job, turn, CallerContext::ROOT)
        .unwrap();
    let answered = TurnOutcome::Answered {
        text: "answer to second".into(),
        stop: AnswerStop::EndTurn,
    };
    session.end_turn(id, Some(answered), TurnEnd::Completed);

    sessions
        .configure(SessionConfigureParams {
            session: session_to_wire(session.id.clone()),
            change: SessionChange::SelectNode {
                node: first.as_str().to_owned(),
            },
        })
        .await
        .unwrap();
    let runtime = session.runtime.lock().await;
    let (reopened, _) = opening_feed(
        &sessions.jobs.get(&session.job).unwrap(),
        runtime.as_ref().unwrap(),
        &Config::default(),
        /*auths*/ &[],
    );

    let replayed: Vec<_> = reopened
        .reset_replay_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::UserMessage { content, .. } => match content.as_slice() {
                [ContentBlock::Text { text }] => Some(Replayed::User(text.clone())),
                _ => None,
            },
            wire::SessionEvent::TurnEnded {
                outcome: TurnOutcome::Answered { text, .. },
                ..
            } => Some(Replayed::Answer(text)),
            _ => None,
        })
        .collect();
    assert_eq!(replayed, vec![Replayed::User("first".into())]);
}
