use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;

// Covers: which answers deliver, which are retried (they may pass), and
// which stop the watch at once (asking again cannot change them; a
// redirect is never followed).
// Owner: webhook delivery verdicts.
#[test]
fn answers_deliver_retry_or_stall() {
    let cases = [
        (Answer::Status(200), Verdict::Delivered(200)),
        (Answer::Status(204), Verdict::Delivered(204)),
        (Answer::Status(301), Verdict::Stall),
        (Answer::Status(302), Verdict::Stall),
        (Answer::Status(400), Verdict::Stall),
        (Answer::Status(401), Verdict::Stall),
        (Answer::Status(404), Verdict::Stall),
        (Answer::Status(408), Verdict::Retry),
        (Answer::Status(429), Verdict::Retry),
        (Answer::Status(500), Verdict::Retry),
        (Answer::Status(503), Verdict::Retry),
        (Answer::Error("connection refused".into()), Verdict::Retry),
    ];
    let verdicts: Vec<(Answer, Verdict)> = cases
        .iter()
        .map(|(answer, _)| (answer.clone(), verdict(answer)))
        .collect();
    assert_eq!(verdicts, cases.to_vec());
}

// Covers: retry gaps double up to the cap and stop once the next retry
// would start past the horizon, so a failing receiver gets a bounded
// number of billed POSTs an hour.
// Owner: webhook retry schedule.
#[test]
fn retry_gaps_double_to_the_cap_and_stop_at_the_horizon() {
    let secs = Duration::from_secs;
    let schedule = RetrySchedule {
        first_gap: secs(2),
        max_gap: secs(10),
        horizon: secs(40),
    };
    // 2 + 4 + 8 + 10 + 10 = 34; one more 10 would start at 44.
    assert_eq!(
        schedule.gaps().collect::<Vec<_>>(),
        vec![secs(2), secs(4), secs(8), secs(10), secs(10)]
    );
}
