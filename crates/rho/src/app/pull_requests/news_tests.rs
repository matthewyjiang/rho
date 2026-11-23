use pretty_assertions::assert_eq;
use rho_protocol::display::NotificationTone;

use super::{omitted, prompt, section, snippet, MODEL_NOTIFICATION_BYTES};
use crate::app::pull_requests::{
    model::{PrId, RepoId},
    state::{Change, CheckRef, End, Failure, Gate, News, Remark},
};

fn news(number: u32, changes: Vec<Change>, end: Option<End>) -> News {
    News {
        seq: 0,
        pr: PrId {
            repo: RepoId {
                host: "github.com".into(),
                owner: "acme".into(),
                name: "widgets".into(),
            },
            number,
        },
        url: format!("https://github.com/acme/widgets/pull/{number}"),
        head: Some("1a2b3c4d5e6f".into()),
        changes,
        end,
        at: 0,
        pushed: false,
        checks: None,
    }
}

fn failed(name: &str) -> CheckRef {
    CheckRef {
        name: name.into(),
        url: None,
        failure: Failure::Failed,
    }
}

fn remark(body: &str) -> Remark {
    Remark {
        author: "alice".into(),
        association: Some("MEMBER".into()),
        path: Some("src/lib.rs".into()),
        body: body.into(),
        verdict: None,
        url: "https://github.com/acme/widgets/pull/12#discussion_r1".into(),
        at: 0,
    }
}

// Covers: a card's color says the most pressing thing that happened.
// Owner: news rendering.
#[test]
fn a_cards_tone_is_its_most_pressing_outcome() {
    let remarks = || Change::Remarks {
        remarks: vec![remark("looks good")],
    };
    let merged = End::Merged {
        by: Some("bob".into()),
        commit: None,
        base: "main".into(),
    };
    let cases = [
        (
            "checks failed",
            news(
                12,
                vec![Change::ChecksFailed {
                    failed: vec![failed("ci")],
                }],
                None,
            ),
        ),
        (
            "checks passed",
            news(
                12,
                vec![Change::ChecksPassed {
                    count: 3,
                    gate: Gate::All,
                }],
                None,
            ),
        ),
        (
            "owed checks finished, one failing",
            news(
                12,
                vec![Change::ChecksFinished {
                    passed: 2,
                    failing: vec![failed("lint")],
                }],
                None,
            ),
        ),
        ("remarks", news(12, vec![remarks()], None)),
        (
            "remarks with a conflict",
            news(12, vec![remarks(), Change::Conflicting], None),
        ),
        ("merged", news(12, vec![], Some(merged.clone()))),
        (
            "merged with remarks",
            news(12, vec![remarks()], Some(merged)),
        ),
        ("closed", news(12, vec![], Some(End::Closed))),
        (
            "comment loop",
            news(12, vec![remarks()], Some(End::CommentLoop)),
        ),
        (
            "unreadable",
            news(12, vec![], Some(End::Unreadable("Not Found".into()))),
        ),
    ];
    let tones: Vec<(&str, NotificationTone)> = cases
        .iter()
        .map(|(name, news)| (*name, news.card().tone))
        .collect();
    assert_eq!(
        tones,
        vec![
            ("checks failed", NotificationTone::Error),
            ("checks passed", NotificationTone::Success),
            ("owed checks finished, one failing", NotificationTone::Error),
            ("remarks", NotificationTone::Accent),
            ("remarks with a conflict", NotificationTone::Error),
            ("merged", NotificationTone::Success),
            ("merged with remarks", NotificationTone::Accent),
            ("closed", NotificationTone::Warning),
            ("comment loop", NotificationTone::Warning),
            ("unreadable", NotificationTone::Warning),
        ]
    );
}

// Covers: a full queue's wake stays within budget and says what it left out.
// Owner: news rendering.
#[test]
fn many_updates_fit_the_budget_and_count_what_is_left_out() {
    let all: Vec<News> = (1..=60)
        .map(|number| {
            let remarks = (0..12).map(|_| remark(&"🦀 word ".repeat(80))).collect();
            news(number, vec![Change::Remarks { remarks }], None)
        })
        .collect();

    let model = prompt(&all, /*dropped*/ 0);

    assert!(
        model.len() <= MODEL_NOTIFICATION_BYTES,
        "{}-byte wake exceeded the {MODEL_NOTIFICATION_BYTES}-byte budget",
        model.len()
    );
    let shown = all
        .iter()
        .take_while(|news| model.contains(&section(news)))
        .count();
    assert!((1..all.len()).contains(&shown), "{shown} of 60 shown");
    assert!(
        model.contains(&omitted(all.len() - shown)),
        "the wake doesn't say {} updates were left out",
        all.len() - shown
    );
}

// Covers: quoted comments lose rendered HTML but keep code as written.
// Owner: news rendering (port of t3's snippet).
#[test]
fn snippets_drop_markup_outside_code() {
    let cases: Vec<(String, String)> = [
        (
            "<!-- bot state -->Looks <b>good</b> to me",
            "Looks good to me",
        ),
        (
            "<details><summary>Log</summary>\n\nbad\n</details>",
            "Log bad",
        ),
        (
            "use `<b>` and <a href=\"x>y\">a link</a>",
            "use `<b>` and a link",
        ),
        ("```\n<div>raw</div>\n```", "``` <div>raw</div> ```"),
        (
            "returns Vec<code> and Promise<T>",
            "returns Vec<code> and Promise<T>",
        ),
        ("a\n\n    <b>indented</b>", "a <b>indented</b>"),
        // A bot review's opening: quote marker, bold, a commit link.
        (
            "> ✅ No new issues found. **Reviewed changes** Reviewed [dbf19a9](https://github.com/o/r/commit/dbf19a9e).",
            "✅ No new issues found. Reviewed changes Reviewed dbf19a9.",
        ),
        ("## Summary\n![shot](https://x/y.png) see [docs]", "Summary shot see [docs]"),
    ]
    .into_iter()
    .map(|(body, want)| (body.to_owned(), want.to_owned()))
    // 299 characters, cut to 197 and an ellipsis.
    .chain([(
        "word ".repeat(60),
        format!("{}...", &"word ".repeat(40)[..197]),
    )])
    .collect();
    let snippets: Vec<(String, String)> = cases
        .iter()
        .map(|(body, _)| (body.clone(), snippet(body)))
        .collect();
    assert_eq!(snippets, cases);
}
