use std::{collections::VecDeque, sync::Mutex};

use pretty_assertions::assert_eq;
use rho_protocol::{Ask, InputRequest, InputRequestId, Question, QuestionAsk, Selection, TurnId};
use serde_json::json;

use super::*;

/// Question `id`, asked by the turn.
fn question(id: u64) -> InputRequest {
    InputRequest {
        id: InputRequestId(id),
        ask: Ask::Question(QuestionAsk {
            title: "Color".into(),
            questions: vec![Question {
                id: "color".into(),
                prompt: "Which color?".into(),
                header: None,
                choices: Vec::new(),
                selection: Selection::One,
                allow_other: true,
                help: None,
                default: None,
                required: true,
            }],
        }),
        tool_call: None,
        timeout: None,
        asker: rho_protocol::InputAsker::Turn,
    }
}

/// One step of a scripted follow.
#[derive(Clone)]
enum Step {
    /// What the follow's next read returns.
    Read(Followed),
    /// The follow reads a live event that is no news, and reads on.
    Live,
}

/// News of turn `turn` in `state`, with requests `inputs` open.
fn news(state: JobState, turn: &str, inputs: &[u64]) -> WaitResult {
    WaitResult {
        session: SessionId::new("s"),
        state,
        done: true,
        outcome: None,
        turn: Some(TurnId::new(turn)),
        inputs: inputs.iter().copied().map(question).collect(),
        cursor: None,
    }
}

fn settled(turn: &str) -> Step {
    Step::Read(Followed::News(news(JobState::Completed, turn, &[])))
}

fn asks(turn: &str, inputs: &[u64]) -> Step {
    Step::Read(Followed::News(news(
        JobState::WaitingForInput,
        turn,
        inputs,
    )))
}

fn detached(reason: DetachReason) -> Step {
    Step::Read(Followed::Ended(FollowEnd::Detached(reason)))
}

/// A scripted session and receiver.
#[derive(Default)]
struct Fake {
    /// What each follow reads, in order of opening.
    follows: Mutex<VecDeque<Vec<Step>>>,
    opened: Mutex<Vec<Reconnect>>,
    /// Each authorization's verdict; allowed once these run out.
    denials: Mutex<VecDeque<Denied>>,
    /// Each POST's answer; 200 once these run out.
    answers: Mutex<VecDeque<Answer>>,
    /// Each POST: the state and the requests its body names.
    posted: Mutex<Vec<(Value, Value)>>,
    slept: Mutex<Vec<Duration>>,
    recorded: Mutex<Vec<String>>,
}

struct FakeFollow {
    steps: VecDeque<Step>,
    progressed: bool,
}

impl FollowPort for FakeFollow {
    async fn next(&mut self) -> Result<Followed, Denied> {
        loop {
            match self.steps.pop_front() {
                Some(Step::Read(read)) => return Ok(read),
                Some(Step::Live) => self.progressed = true,
                None => return Ok(Followed::Ended(FollowEnd::Closed)),
            }
        }
    }

    fn progressed(&self) -> bool {
        self.progressed
    }
}

impl Ports for Fake {
    type Follow = FakeFollow;

    async fn follow(&self, connect: Reconnect) -> Result<FakeFollow, Unfollowed> {
        self.opened.lock().unwrap().push(connect);
        let script = self.follows.lock().unwrap().pop_front().unwrap_or_default();
        Ok(FakeFollow {
            steps: script.into(),
            progressed: false,
        })
    }

    async fn authorize(&self) -> Result<WebhookTarget, Denied> {
        if let Some(denied) = self.denials.lock().unwrap().pop_front() {
            return Err(denied);
        }
        Ok(WebhookTarget {
            name: "hook".into(),
            url: "https://example.com/hook".into(),
            secret: "secret".into(),
            tokens: vec!["bot".into()],
        })
    }

    async fn post(&self, _target: &WebhookTarget, body: &Value) -> Answer {
        let inputs = Value::Array(
            body["inputs"]
                .as_array()
                .map(|inputs| inputs.iter().map(|input| input["id"].clone()).collect())
                .unwrap_or_default(),
        );
        self.posted
            .lock()
            .unwrap()
            .push((body["state"].clone(), inputs));
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Answer::Status(200))
    }

    async fn sleep(&self, gap: Duration) {
        self.slept.lock().unwrap().push(gap);
    }

    async fn record(&self, record: Record<'_>) {
        let line = match record {
            Record::Delivered { reported, status } => {
                format!("delivered {:?} {status}", reported.inputs)
            }
            Record::Failed { error } => format!("failed {error}"),
        };
        self.recorded.lock().unwrap().push(line);
    }
}

/// What a watcher did.
#[derive(Debug, PartialEq)]
struct Ran {
    end: End,
    posted: Vec<(Value, Value)>,
    opened: Vec<Reconnect>,
    slept: usize,
    recorded: Vec<String>,
}

/// One scripted watch.
struct Case {
    name: &'static str,
    reported: Option<NewsKey>,
    follows: Vec<Vec<Step>>,
    denials: Vec<Denied>,
    answers: Vec<Answer>,
    expected: Ran,
}

// Covers: a watch POSTs each new piece of news once, ends after the
// settle is delivered, reopens a follow the daemon dropped without
// reposting what it reported, stalls a follow the daemon keeps dropping
// before it reads anything past its replay but reopens one that read on
// each time, drops a watch whose session or token is gone without
// POSTing, stalls when the daemon stops or the receiver refuses, and
// retries a receiver that may recover until it takes it or the schedule
// runs out.
// Owner: webhook watcher loop.
#[tokio::test]
async fn a_watcher_delivers_each_piece_of_news_once() {
    let secs = Duration::from_secs;
    let schedule = RetrySchedule {
        first_gap: secs(60),
        max_gap: secs(60),
        horizon: secs(120),
    };
    let completed = || (json!("completed"), json!([]));
    let waiting = |ids: Value| (json!("waiting_for_input"), ids);
    let ran = |end, posted, opened, slept, recorded: &[&str]| Ran {
        end,
        posted,
        opened,
        slept,
        recorded: recorded.iter().map(|line| (*line).to_owned()).collect(),
    };
    let cases = [
        Case {
            name: "settled",
            reported: None,
            follows: vec![vec![settled("t1")]],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Delivered,
                vec![completed()],
                vec![Reconnect::First],
                0,
                &["delivered [] 200"],
            ),
        },
        Case {
            name: "asks, is answered, then settles",
            reported: None,
            follows: vec![vec![asks("t1", &[3]), asks("t1", &[]), settled("t1")]],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Delivered,
                vec![waiting(json!([3])), completed()],
                vec![Reconnect::First],
                0,
                &["delivered [InputRequestId(3)] 200", "delivered [] 200"],
            ),
        },
        Case {
            name: "the request the watch call returned",
            reported: Some(NewsKey::of(&news(JobState::WaitingForInput, "t1", &[3]))),
            follows: vec![vec![asks("t1", &[3]), settled("t1")]],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Delivered,
                vec![completed()],
                vec![Reconnect::First],
                0,
                &["delivered [] 200"],
            ),
        },
        Case {
            name: "lagged, and the reopen replays what was reported",
            reported: None,
            follows: vec![
                vec![asks("t1", &[3]), detached(DetachReason::Lagged)],
                vec![asks("t1", &[3]), settled("t1")],
            ],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Delivered,
                vec![waiting(json!([3])), completed()],
                vec![Reconnect::First, Reconnect::Running],
                0,
                &["delivered [InputRequestId(3)] 200", "delivered [] 200"],
            ),
        },
        Case {
            name: "the daemon keeps dropping the follow after its replay",
            reported: None,
            follows: vec![vec![asks("t1", &[3]), detached(DetachReason::NotLoaded)]; 4],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Stalled(
                    "the rho daemon ended 4 follows in a row before they read anything new, the last NotLoaded; a watch reopens at most MAX_REOPENS_WITHOUT_PROGRESS (3) times without progress"
                        .into(),
                ),
                vec![waiting(json!([3]))],
                vec![
                    Reconnect::First,
                    Reconnect::Running,
                    Reconnect::Running,
                    Reconnect::Running,
                ],
                0,
                &["delivered [InputRequestId(3)] 200"],
            ),
        },
        Case {
            name: "a long turn's follow lags again and again, reading on each time",
            reported: None,
            follows: vec![
                vec![Step::Live, detached(DetachReason::Lagged)],
                vec![Step::Live, detached(DetachReason::Lagged)],
                vec![Step::Live, detached(DetachReason::Lagged)],
                vec![Step::Live, detached(DetachReason::Lagged)],
                vec![settled("t1")],
            ],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Delivered,
                vec![completed()],
                vec![
                    Reconnect::First,
                    Reconnect::Running,
                    Reconnect::Running,
                    Reconnect::Running,
                    Reconnect::Running,
                ],
                0,
                &["delivered [] 200"],
            ),
        },
        Case {
            name: "the daemon restarts in place",
            reported: None,
            follows: vec![
                vec![detached(DetachReason::Restarting)],
                vec![settled("t1")],
            ],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Delivered,
                vec![completed()],
                vec![Reconnect::First, Reconnect::Restarted],
                0,
                &["delivered [] 200"],
            ),
        },
        Case {
            name: "the session is deleted",
            reported: None,
            follows: vec![vec![detached(DetachReason::Deleted)]],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Dropped("session s was deleted".into()),
                Vec::new(),
                vec![Reconnect::First],
                0,
                &[],
            ),
        },
        Case {
            name: "the daemon stops",
            reported: None,
            follows: vec![vec![detached(DetachReason::DaemonStopping)]],
            denials: Vec::new(),
            answers: Vec::new(),
            expected: ran(
                End::Stalled("the rho daemon stopped".into()),
                Vec::new(),
                vec![Reconnect::First],
                0,
                &[],
            ),
        },
        Case {
            name: "the token is revoked before the POST",
            reported: None,
            follows: vec![vec![settled("t1")]],
            denials: vec![Denied::Drop("token \"bot\" was revoked".into())],
            answers: Vec::new(),
            expected: ran(
                End::Dropped("token \"bot\" was revoked".into()),
                Vec::new(),
                vec![Reconnect::First],
                0,
                &[],
            ),
        },
        Case {
            name: "429, then 200",
            reported: None,
            follows: vec![vec![settled("t1")]],
            denials: Vec::new(),
            answers: vec![Answer::Status(429)],
            expected: ran(
                End::Delivered,
                vec![completed(), completed()],
                vec![Reconnect::First],
                1,
                &["delivered [] 200"],
            ),
        },
        Case {
            name: "the receiver refuses",
            reported: None,
            follows: vec![vec![settled("t1")]],
            denials: Vec::new(),
            answers: vec![Answer::Status(401)],
            expected: ran(
                End::Stalled("webhook \"hook\" refused delivery: answer 401".into()),
                vec![completed()],
                vec![Reconnect::First],
                0,
                &["failed webhook \"hook\" refused delivery: answer 401"],
            ),
        },
        Case {
            name: "the schedule runs out",
            reported: None,
            follows: vec![vec![settled("t1")]],
            denials: Vec::new(),
            answers: vec![Answer::Status(503); 3],
            expected: ran(
                End::Stalled(
                    "webhook \"hook\" not delivered after 3 attempts, 2 min of retry gaps between them: last answer 503"
                        .into(),
                ),
                vec![completed(), completed(), completed()],
                vec![Reconnect::First],
                2,
                &["failed webhook \"hook\" not delivered after 3 attempts, 2 min of retry gaps between them: last answer 503"],
            ),
        },
    ];
    for case in cases {
        let fake = Fake {
            follows: Mutex::new(case.follows.into()),
            denials: Mutex::new(case.denials.into()),
            answers: Mutex::new(case.answers.into()),
            ..Fake::default()
        };
        let watching = Watching {
            session: SessionId::new("s"),
            webhook: "hook".into(),
            server: None,
            reported: case.reported,
        };
        let end = run(&fake, schedule, "lab-box", watching).await;
        assert_eq!(
            Ran {
                end,
                posted: fake.posted.into_inner().unwrap(),
                opened: fake.opened.into_inner().unwrap(),
                slept: fake.slept.into_inner().unwrap().len(),
                recorded: fake.recorded.into_inner().unwrap(),
            },
            case.expected,
            "{}",
            case.name
        );
    }
}

// Covers: the webhook body is the news exactly as `wait` returns it, plus
// the server it is from, which a bot driving several machines needs.
// Owner: webhook body wire format.
#[test]
fn the_body_is_the_wait_result_and_the_server() {
    assert_eq!(
        Value::Object(body("lab-box", &news(JobState::Completed, "t1", &[]))),
        json!({
            "server": "lab-box",
            "session": "s",
            "state": "completed",
            "done": true,
            "turn": "t1",
        })
    );
}
