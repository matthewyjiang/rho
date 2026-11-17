use std::{collections::VecDeque, sync::Mutex};

use pretty_assertions::assert_eq;
use rho_protocol::{JobState, TurnId};

use super::*;

/// A remote that answers each `wait` from a script, recording the cursor
/// each was called from and every sleep.
#[derive(Default)]
struct Scripted {
    answers: Mutex<VecDeque<Result<WaitResult, WaitFailed>>>,
    cursors: Mutex<Vec<Option<String>>>,
    slept: Mutex<Vec<Duration>>,
}

impl RemoteWaits for Scripted {
    async fn wait(&self, cursor: Option<&str>) -> Result<WaitResult, WaitFailed> {
        self.cursors.lock().unwrap().push(cursor.map(str::to_owned));
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script has an answer for every wait")
    }

    async fn sleep(&self, gap: Duration) {
        self.slept.lock().unwrap().push(gap);
    }
}

fn news(state: JobState, done: bool, cursor: &str) -> WaitResult {
    WaitResult {
        session: SessionId::new("s"),
        state,
        done,
        outcome: None,
        turn: Some(TurnId::new("t1")),
        inputs: Vec::new(),
        cursor: Some(cursor.to_owned()),
    }
}

/// Retries a second apart, two of them.
const SCHEDULE: RetrySchedule = RetrySchedule {
    first_gap: Duration::from_secs(1),
    max_gap: Duration::from_secs(1),
    horizon: Duration::from_secs(2),
};

/// What a follow did: what each `next` returned, the cursors it waited
/// from, and how long it slept between.
#[derive(Debug, PartialEq, Eq)]
struct Followed {
    returned: Vec<Result<JobState, Denied>>,
    cursors: Vec<Option<String>>,
    slept: Vec<Duration>,
}

async fn follow(script: Vec<Result<WaitResult, WaitFailed>>, nexts: usize) -> Followed {
    let mut follow = RemoteFollow::new(
        Scripted {
            answers: Mutex::new(script.into()),
            ..Scripted::default()
        },
        SCHEDULE,
    );
    let mut returned = Vec::new();
    for _ in 0..nexts {
        returned.push(follow.next().await.map(|news| news.state));
    }
    Followed {
        returned,
        cursors: follow.waits.cursors.into_inner().unwrap(),
        slept: follow.waits.slept.into_inner().unwrap(),
    }
}

// Covers: a remote follow resumes each `wait` from the cursor the last
// returned; asks again at once while the turn makes progress, but only
// after IDLE_POLL when a `wait` brings nothing new, as a session waiting
// for a person repeating its news or a dropped follow from the same
// cursor; retries an unreachable remote on its schedule and stalls once
// that runs out; and drops the watch when the remote ends it.
// Owner: hub webhook watches of remote sessions.
#[tokio::test]
async fn a_remote_follow_reports_each_new_news_and_backs_off_on_none() {
    let idle = IDLE_POLL;
    let second = Duration::from_secs(1);
    let unreachable = || Err(WaitFailed::Again("down".to_owned()));
    let cases = [
        (
            "progress, then settles",
            vec![
                Ok(news(JobState::Running, false, "c:1")),
                Ok(news(JobState::Completed, true, "c:2")),
            ],
            1,
            Followed {
                returned: vec![Ok(JobState::Completed)],
                cursors: vec![None, Some("c:1".to_owned())],
                slept: Vec::new(),
            },
        ),
        (
            "waits for a person, repeats it, then settles",
            vec![
                Ok(news(JobState::WaitingForInput, true, "c:1")),
                Ok(news(JobState::WaitingForInput, true, "c:1")),
                Ok(news(JobState::Completed, true, "c:3")),
            ],
            2,
            Followed {
                returned: vec![Ok(JobState::WaitingForInput), Ok(JobState::Completed)],
                cursors: vec![None, Some("c:1".to_owned()), Some("c:1".to_owned())],
                slept: vec![idle],
            },
        ),
        (
            "nothing new from the same cursor",
            vec![
                Ok(news(JobState::Running, false, "c:1")),
                Ok(news(JobState::Running, false, "c:1")),
                Ok(news(JobState::Completed, true, "c:2")),
            ],
            1,
            Followed {
                returned: vec![Ok(JobState::Completed)],
                cursors: vec![None, Some("c:1".to_owned()), Some("c:1".to_owned())],
                slept: vec![idle],
            },
        ),
        (
            "unreachable, then back",
            vec![unreachable(), Ok(news(JobState::Completed, true, "c:1"))],
            1,
            Followed {
                returned: vec![Ok(JobState::Completed)],
                cursors: vec![None, None],
                slept: vec![second],
            },
        ),
        (
            "unreachable past the schedule",
            vec![unreachable(), unreachable(), unreachable()],
            1,
            Followed {
                returned: vec![Err(Denied::Stall("down, after 3 attempts".to_owned()))],
                cursors: vec![None, None, None],
                slept: vec![second, second],
            },
        ),
        (
            "ended by the remote",
            vec![Err(WaitFailed::Ended("no session s".to_owned()))],
            1,
            Followed {
                returned: vec![Err(Denied::Drop("no session s".to_owned()))],
                cursors: vec![None],
                slept: Vec::new(),
            },
        ),
    ];
    for (name, script, nexts, expected) in cases {
        assert_eq!(follow(script, nexts).await, expected, "{name}");
    }
}

// Covers: a remote that is down, slow, refusing the secret, as while `rho
// mcp remote add` rotates it, or answering an error that may pass, as with
// its daemon down, is asked again; one that answers an error it marks
// lasting, as for a session it does not know, ends the follow.
// Owner: hub webhook watches of remote sessions.
#[test]
fn only_an_answered_error_ends_a_remote_follow() {
    let server = || "lab-box".to_owned();
    let errors = [
        RemoteError::Unreachable {
            server: server(),
            detail: "connection refused".to_owned(),
        },
        RemoteError::Refused { server: server() },
        RemoteError::Failed {
            server: server(),
            message: "could not reach the rho daemon".to_owned(),
            lasts: Lasts::ForNow,
        },
        RemoteError::Failed {
            server: server(),
            message: "no session s is known to the rho daemon".to_owned(),
            lasts: Lasts::ForGood,
        },
    ];
    let ended: Vec<bool> = errors
        .into_iter()
        .map(|error| matches!(WaitFailed::from(error), WaitFailed::Ended(_)))
        .collect();
    assert_eq!(ended, vec![false, false, false, true]);
}
