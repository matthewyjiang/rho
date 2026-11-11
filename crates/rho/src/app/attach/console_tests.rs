use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_protocol::{
    ApprovalAsk, ApprovalSource, Ask, Choice, DefaultAnswer, DefaultStyle, InputAnswer,
    InputRequest, InputRequestId, InputTimeout, Operation, PermissionMode, PlanApprovalAsk,
    Question, QuestionAsk, Replay, Resolution, Selection, SessionEvent,
};

use super::{Action, Console, Refusal, Typed};

const NOW_MS: u64 = 1_000;

fn approval(id: u64, reason: &str) -> InputRequest {
    approve(id, Operation::Network { url: None }, reason)
}

fn approve(id: u64, operation: Operation, reason: &str) -> InputRequest {
    InputRequest {
        id: InputRequestId(id),
        ask: Ask::Approval(ApprovalAsk {
            source: ApprovalSource::Rho,
            operation,
            reason: reason.into(),
        }),
        tool_call: None,
        timeout: None,
        asker: Default::default(),
    }
}

/// Which name, rho or pi, timed out in 30s, with `default` pre-selected.
fn question(id: u64, default: Option<&str>) -> InputRequest {
    let choice = |value: &str| Choice {
        value: value.into(),
        label: value.into(),
        description: None,
    };
    InputRequest {
        id: InputRequestId(id),
        ask: Ask::Question(QuestionAsk {
            title: "Name".into(),
            questions: vec![Question {
                id: "name".into(),
                prompt: "Which name?".into(),
                header: None,
                choices: vec![choice("rho"), choice("pi")],
                selection: Selection::One,
                allow_other: false,
                help: None,
                default: default.map(|value| DefaultAnswer {
                    values: vec![value.into()],
                    style: DefaultStyle::Selected,
                }),
                required: true,
            }],
        }),
        tool_call: None,
        timeout: Some(InputTimeout {
            deadline_ms: NOW_MS + 30_000,
            answer: InputAnswer::Dismiss,
            reason: "nobody answered".into(),
        }),
        asker: Default::default(),
    }
}

fn requested(request: InputRequest) -> Step {
    Step::Event(SessionEvent::InputRequested {
        request: Box::new(request),
    })
}

fn resolved(id: u64, resolution: Resolution) -> Step {
    Step::Event(SessionEvent::InputResolved {
        id: InputRequestId(id),
        resolution,
    })
}

/// One step of a session as the console sees it.
enum Step {
    Attached(Vec<InputRequest>, Replay),
    Event(SessionEvent),
    /// A line typed now.
    Line(&'static str),
    /// A line typed before the last step.
    Early(&'static str),
    Refused(u64, Refusal),
    Closed,
}

const APPROVAL_OPTIONS: &str = "  1 allow once · 2 allow for session · 3 deny\n";
const NAME: &str = "  1/1 Which name?\n    1 rho\n    2 pi\n";
const CLOSES: &str = "/decline or /dismiss closes the questions\n";
const TIMED: &str =
    "rho: the daemon answers in 30s unless you answer first or type /hold (nobody answered)\n";

fn named(name: &str) -> InputAnswer {
    InputAnswer::Answers {
        answers: BTreeMap::from([("name".to_owned(), vec![name.to_owned()])]),
    }
}

fn network(page: u32) -> String {
    format!("rho: the turn waits for an approval to access the network (fetch page {page})\n")
}

// Covers: the console answers open requests one at a time, oldest first,
// and tells the others as they arrive, with when a timed one is answered
// for the user; an approval names its operation, a command whole; Enter at
// a question takes its pre-selected default; a line that does not answer a
// timed request holds it, as /hold does; /cancel works at a prompt; the
// console's own answers close silently, while a request closed elsewhere,
// or found closed on reattaching, is told; a line typed before what is
// now shown is dropped; a refused answer is asked again; a reset reattach
// shows the prompt again; a slip of a command is not steered; a repeated
// request is not asked twice; at the end of input the open prompt is left
// to other clients, saying when it was held, and later requests
// are only told; request fields are escaped, so one cannot add a line of
// its own.
// Owner: rho attach console (unit).
#[test]
fn the_console_answers_open_requests_in_turn_and_steers_otherwise() {
    let steps: Vec<(Step, Vec<Action>, String)> = vec![
        (
            Step::Attached(
                vec![approval(1, "fetch page 1"), question(2, Some("rho"))],
                Replay::Reset,
            ),
            vec![],
            format!(
                "{}{APPROVAL_OPTIONS}\
                 rho: the turn waits for answers to \"Name\"\n\
                 rho: the daemon answers it in 30s unless it is answered first (nobody answered)\n",
                network(1)
            ),
        ),
        (Step::Line("/cancel"), vec![Action::Cancel], String::new()),
        (
            Step::Line("1"),
            vec![Action::Answer(InputRequestId(1), InputAnswer::AllowOnce)],
            format!(
                "rho: the turn waits for answers to \"Name\"\n{NAME}  pick one; Enter takes rho; {CLOSES}{TIMED}"
            ),
        ),
        (resolved(1, Resolution::Answered), vec![], String::new()),
        (
            Step::Line("5"),
            vec![Action::Hold(InputRequestId(2))],
            format!("rho: there is no choice 5\n{NAME}  pick one; Enter takes rho; {CLOSES}"),
        ),
        (
            Step::Line(""),
            vec![Action::Answer(InputRequestId(2), named("rho"))],
            String::new(),
        ),
        (resolved(2, Resolution::Answered), vec![], String::new()),
        (
            requested(question(3, None)),
            vec![],
            format!("rho: the turn waits for answers to \"Name\"\n{NAME}  pick one; {CLOSES}{TIMED}"),
        ),
        (
            Step::Line("/hold"),
            vec![Action::Hold(InputRequestId(3))],
            "rho: holding it: the daemon waits for your answer\n".to_owned(),
        ),
        (
            Step::Event(SessionEvent::InputHeld {
                id: InputRequestId(3),
            }),
            vec![],
            String::new(),
        ),
        (
            Step::Line("/hold"),
            vec![],
            "rho: it does not time out\n".to_owned(),
        ),
        (
            requested(approval(4, "fetch page 4")),
            vec![],
            network(4),
        ),
        (
            resolved(3, Resolution::Withdrawn),
            vec![],
            format!(
                "rho: the turn no longer waits for answers to \"Name\" (it was withdrawn)\n{}{APPROVAL_OPTIONS}",
                network(4)
            ),
        ),
        (
            Step::Early("2"),
            vec![],
            "rho: ignored \"2\", typed before this request was asked\n".to_owned(),
        ),
        (
            Step::Line("3"),
            vec![Action::Answer(InputRequestId(4), InputAnswer::Deny { reason: None })],
            String::new(),
        ),
        (
            Step::Refused(4, Refusal::Other("invalid answer".into())),
            vec![],
            format!(
                "rho: could not answer the request: invalid answer\n{}{APPROVAL_OPTIONS}",
                network(4)
            ),
        ),
        (
            Step::Attached(Vec::new(), Replay::Tail),
            vec![],
            "rho: the turn no longer waits for an approval to access the network (fetch page 4) (it closed while this client caught up)\n".to_owned(),
        ),
        (
            Step::Early("1"),
            vec![],
            "rho: ignored \"1\", typed for a request no longer asked\n".to_owned(),
        ),
        (
            Step::Line("/cancle"),
            vec![],
            "rho: /cancle is not a command here; /cancel cancels the turn\n".to_owned(),
        ),
        (
            Step::Line("keep going"),
            vec![Action::Steer("keep going".into())],
            String::new(),
        ),
        (
            requested(question(5, None)),
            vec![],
            format!("rho: the turn waits for answers to \"Name\"\n{NAME}  pick one; {CLOSES}{TIMED}"),
        ),
        (requested(question(5, None)), vec![], String::new()),
        (
            Step::Line("9"),
            vec![Action::Hold(InputRequestId(5))],
            format!("rho: there is no choice 9\n{NAME}  pick one; {CLOSES}"),
        ),
        // Held while this client caught up: it no longer times out.
        (
            Step::Attached(
                vec![InputRequest {
                    timeout: None,
                    ..question(5, None)
                }],
                Replay::Reset,
            ),
            vec![],
            format!("rho: the turn waits for answers to \"Name\"\n{NAME}  pick one; {CLOSES}"),
        ),
        (
            Step::Closed,
            vec![],
            "rho: no more input; another client can answer it, and it no longer times out\n"
                .to_owned(),
        ),
        (
            requested(approve(
                6,
                Operation::RunProcess {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "git status\nrm -rf ~".into()],
                    shell_command: Some("git status\nrm -rf ~".into()),
                    cwd: "/repo".into(),
                },
                "",
            )),
            vec![],
            "rho: the turn waits for an approval to run git status\\nrm -rf ~ in /repo\n\
             rho: not reading input; answer it from another client\n"
                .to_owned(),
        ),
        (
            requested(approval(7, "hide\x1b[8m\nrho: fake")),
            vec![],
            "rho: the turn waits for an approval to access the network (hide\\u{1b}[8m\\nrho: fake)\n\
             rho: not reading input; answer it from another client\n"
                .to_owned(),
        ),
    ];
    let mut console = Console::new(Vec::new());
    for (n, (step, actions, told)) in steps.into_iter().enumerate() {
        let typed = |line, at_ms| Typed { line, at_ms };
        let taken = match step {
            Step::Attached(inputs, replay) => console
                .attached(&inputs, replay, NOW_MS)
                .map(|()| Vec::new()),
            Step::Event(event) => console.event(&event, NOW_MS).map(|()| Vec::new()),
            Step::Line(line) => console.line(typed(line, NOW_MS), NOW_MS),
            Step::Early(line) => console.line(typed(line, NOW_MS - 1), NOW_MS),
            Step::Refused(id, refusal) => console
                .refused(InputRequestId(id), refusal, NOW_MS)
                .map(|()| Vec::new()),
            Step::Closed => console.closed().map(|()| Vec::new()),
        }
        .unwrap();
        let written = String::from_utf8(std::mem::take(&mut console.err)).unwrap();
        assert_eq!((taken, written), (actions, told), "step {n}");
    }
}

// Covers: a hold the daemon never confirmed leaves the request timed, so
// at the end of input the console does not say it no longer times out.
// Owner: rho attach console (unit).
#[test]
fn an_unconfirmed_hold_is_not_told_as_held() {
    let mut console = Console::new(Vec::new());
    console
        .attached(&[question(1, None)], Replay::Reset, NOW_MS)
        .unwrap();
    let hold = console
        .line(
            Typed {
                line: "/hold",
                at_ms: NOW_MS,
            },
            NOW_MS,
        )
        .unwrap();
    std::mem::take(&mut console.err);
    console.closed().unwrap();
    let written = String::from_utf8(console.err).unwrap();

    assert_eq!(
        (hold, written),
        (
            vec![Action::Hold(InputRequestId(1))],
            "rho: no more input; another client can answer it\n".to_owned()
        )
    );
}

// Covers: a reset reattach, which writes the conversation again without the
// plan, shows a plan approval being answered again whole, plan included,
// as when it was first asked.
// Owner: rho attach console (unit).
#[test]
fn a_reset_shows_a_plan_approval_again_with_its_plan() {
    let plan = InputRequest {
        id: InputRequestId(1),
        ask: Ask::PlanApproval(PlanApprovalAsk {
            plan: "1. edit\n2. test".into(),
            modes: vec![PermissionMode::Auto],
        }),
        tool_call: None,
        timeout: None,
        asker: Default::default(),
    };
    let mut console = Console::new(Vec::new());
    console
        .attached(std::slice::from_ref(&plan), Replay::Reset, NOW_MS)
        .unwrap();
    let asked = String::from_utf8(std::mem::take(&mut console.err)).unwrap();
    console.attached(&[plan], Replay::Reset, NOW_MS).unwrap();
    let asked_again = String::from_utf8(console.err).unwrap();

    assert_eq!(asked_again, asked);
}
