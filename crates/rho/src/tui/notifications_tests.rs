use pretty_assertions::assert_eq;

use super::{NotificationChannel, TerminalNotifier};
use crate::herdr::HerdrState;

/// Environment variables visible to `detect`.
type Vars = &'static [(&'static str, &'static str)];

#[test]
fn detect_prefers_bell_inside_multiplexers() {
    let cases: [(&str, Vars, NotificationChannel); 7] = [
        (
            "iterm",
            &[("TERM_PROGRAM", "iTerm.app")],
            NotificationChannel::Osc9,
        ),
        (
            "ghostty term",
            &[("TERM", "xterm-ghostty")],
            NotificationChannel::Osc9,
        ),
        (
            "kitty",
            &[("KITTY_WINDOW_ID", "1")],
            NotificationChannel::Osc9,
        ),
        (
            "tmux inside kitty",
            &[
                ("KITTY_WINDOW_ID", "1"),
                ("TMUX", "/tmp/tmux-1/default,1,0"),
            ],
            NotificationChannel::Bell,
        ),
        (
            "screen inside wezterm",
            &[("TERM_PROGRAM", "WezTerm"), ("STY", "1.pts-0")],
            NotificationChannel::Bell,
        ),
        (
            "apple terminal",
            &[("TERM_PROGRAM", "Apple_Terminal")],
            NotificationChannel::Bell,
        ),
        ("unknown", &[], NotificationChannel::Bell),
    ];
    for (case, vars, expected) in cases {
        let channel = NotificationChannel::detect(|key| {
            vars.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        });
        assert_eq!(channel, expected, "{case}");
    }
}

#[test]
fn osc9_body_cannot_terminate_the_sequence() {
    assert_eq!(
        NotificationChannel::Osc9.encode("blocked\x07\x1b]9;spoof\nnext"),
        b"\x1b]9;rho: blocked  ]9;spoof next\x07".to_vec()
    );
}

#[derive(Clone, Copy)]
enum Step {
    Focus(bool),
    State(HerdrState, Option<&'static str>),
    /// The event loop is about to wait for input.
    Idle,
}

struct NotifyCase {
    name: &'static str,
    channel: Option<NotificationChannel>,
    steps: Vec<Step>,
    expected: Vec<&'static str>,
}

#[test]
fn notifies_once_per_wait_for_the_user() {
    use HerdrState::{Blocked, Idle as Rest, Working};
    use Step::{Focus, Idle, State};
    let approval = State(Blocked, Some("waiting for approval"));
    let osc9 = Some(NotificationChannel::Osc9);
    let case = |name, channel, steps, expected| NotifyCase {
        name,
        channel,
        steps,
        expected,
    };
    let cases = [
        case(
            "finished turn while away",
            osc9,
            vec![
                Focus(false),
                State(Working, None),
                State(Rest, None),
                Idle,
                Idle,
            ],
            vec!["turn finished"],
        ),
        case(
            "goal turns notify once at the end",
            osc9,
            vec![
                Focus(false),
                State(Working, None),
                State(Rest, None),
                State(Working, None),
                State(Rest, None),
                Idle,
            ],
            vec!["turn finished"],
        ),
        case(
            "focused user is not interrupted",
            osc9,
            vec![
                State(Working, None),
                approval,
                State(Working, None),
                State(Rest, None),
                Idle,
            ],
            vec![],
        ),
        case(
            "focus returns before the loop idles",
            osc9,
            vec![
                Focus(false),
                State(Working, None),
                State(Rest, None),
                Focus(true),
                Idle,
            ],
            vec![],
        ),
        case(
            "approval notifies at once and only once",
            osc9,
            vec![Focus(false), State(Working, None), approval, approval],
            vec!["waiting for approval"],
        ),
        case(
            "blocked rest replaces the finished turn",
            osc9,
            vec![
                Focus(false),
                State(Working, None),
                State(Blocked, Some("goal blocked")),
                Idle,
            ],
            vec!["goal blocked"],
        ),
        case(
            "rest without a turn is silent",
            osc9,
            vec![Focus(false), State(Rest, None), Idle],
            vec![],
        ),
        case(
            "herdr owns notifications",
            None,
            vec![
                Focus(false),
                State(Working, None),
                approval,
                State(Rest, None),
                Idle,
            ],
            vec![],
        ),
    ];
    for NotifyCase {
        name,
        channel,
        steps,
        expected,
    } in cases
    {
        let mut notifier = TerminalNotifier::new(channel);
        let mut sent = Vec::new();
        for step in steps {
            let bytes = match step {
                Focus(focused) => {
                    notifier.set_focused(focused);
                    None
                }
                State(state, message) => notifier.observe(state, message),
                Idle => notifier.take_ready(),
            };
            sent.extend(bytes);
        }
        let expected: Vec<Vec<u8>> = expected
            .into_iter()
            .map(|body| NotificationChannel::Osc9.encode(body))
            .collect();
        assert_eq!(sent, expected, "{name}");
    }
}
