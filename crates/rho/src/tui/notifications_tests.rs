use crossterm::event::Event;
use pretty_assertions::assert_eq;

use super::{NotificationChannel, TerminalNotifier};
use crate::tui::UserWait;

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

#[derive(Clone)]
enum Step {
    Terminal(Event),
    TurnFinished,
    Wait(UserWait),
    /// The event loop is about to wait for input.
    Idle,
}

#[test]
fn notifies_unfocused_user_once_per_wait() {
    use Step::{Idle, Terminal, TurnFinished, Wait};
    let away = Terminal(Event::FocusLost);
    let back = Terminal(Event::FocusGained);
    let approval = Wait(UserWait::Approval);
    let cases: [(&str, Vec<Step>, Vec<&str>); 5] = [
        (
            "finished turn while away",
            vec![away.clone(), TurnFinished, Idle, Idle],
            vec!["turn finished"],
        ),
        (
            "goal turns notify once at the end",
            vec![away.clone(), TurnFinished, TurnFinished, Idle],
            vec!["turn finished"],
        ),
        (
            "every approval needs an answer",
            vec![
                away.clone(),
                approval.clone(),
                approval.clone(),
                TurnFinished,
                Idle,
            ],
            vec![
                "waiting for approval",
                "waiting for approval",
                "turn finished",
            ],
        ),
        (
            "focused user is not interrupted",
            vec![approval, TurnFinished, Idle],
            vec![],
        ),
        (
            "focus returns before the loop idles",
            vec![away, TurnFinished, back, Idle],
            vec![],
        ),
    ];
    for (case, steps, expected) in cases {
        let mut notifier = TerminalNotifier::new(NotificationChannel::Osc9);
        let mut sent = Vec::new();
        for step in steps {
            let bytes = match step {
                Terminal(event) => {
                    notifier.observe_focus(&event);
                    None
                }
                TurnFinished => {
                    notifier.turn_finished();
                    None
                }
                Wait(wait) => notifier.user_wait(wait),
                Idle => notifier.take_ready(),
            };
            sent.extend(bytes);
        }
        let expected: Vec<Vec<u8>> = expected
            .into_iter()
            .map(|body| NotificationChannel::Osc9.encode(body))
            .collect();
        assert_eq!(sent, expected, "{case}");
    }
}
