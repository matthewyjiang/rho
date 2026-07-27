use std::time::{Duration, Instant};

use super::super::{StreamKind, StreamUi, STREAM_UI_TICK};
use super::{StreamPacer, MAX_RESERVE_CHARS, TARGET_LEAD};

/// Frame interval the TUI ticks the pacer at.
const TICK: Duration = Duration::from_millis(24);

/// Plays a burst-arrival stream and reports what reached the screen per tick.
///
/// `burst` characters arrive every `flush` interval, matching how a provider
/// flushes its socket.
fn play(burst: usize, flush: Duration, bursts: usize) -> Vec<usize> {
    let start = Instant::now();
    let mut pacer = StreamPacer::default();
    let mut reserve = 0usize;
    let mut released = Vec::new();
    let mut now = start;
    let ticks = (flush.as_secs_f64() / TICK.as_secs_f64()).round() as usize;
    for index in 0..bursts {
        let was_empty = reserve == 0;
        reserve += burst;
        if was_empty {
            pacer.note_refill(now);
        }
        for _ in 0..ticks.max(1) {
            now += TICK;
            let allowance = pacer.release_allowance(now, reserve);
            reserve -= allowance;
            // The very first tick has no interval to bill against when note_refill
            // already stamped last_release at the arrival instant.
            if index > 0 || allowance > 0 {
                released.push(allowance);
            }
        }
    }
    released
}

#[test]
fn spreads_a_burst_across_the_frames_that_follow_it() {
    // grok-4.5 measured: ~13 chars every ~57ms, so a little over two frames.
    let released = play(13, Duration::from_millis(57), 40);
    let steady = &released[released.len() / 2..];

    assert!(
        steady.iter().all(|chars| *chars > 0),
        "every frame should move text, got {steady:?}"
    );
    let largest = steady.iter().copied().max().expect("frames");
    assert!(
        largest < 13,
        "no frame should replay the whole burst, largest was {largest}"
    );
}

#[test]
fn keeps_up_with_the_arrival_rate() {
    let bursts = 60;
    let released: usize = play(13, Duration::from_millis(57), bursts).iter().sum();
    let arrived = 13 * bursts;

    // Playback trails by about the target lead and no more, so text neither
    // piles up without bound nor races ahead of what has arrived.
    assert!(
        released <= arrived,
        "released {released} more than the {arrived} that arrived"
    );
    assert!(
        released * 10 >= arrived * 9,
        "released only {released} of {arrived}, too far behind"
    );
}

#[test]
fn keeps_playing_while_the_provider_stalls() {
    let start = Instant::now();
    let mut pacer = StreamPacer::default();
    let mut now = start;

    // Establish a reserve, then go quiet for far longer than a normal flush.
    let mut reserve = 0usize;
    for _ in 0..20 {
        let was_empty = reserve == 0;
        reserve += 13;
        if was_empty {
            pacer.note_refill(now);
        }
        for _ in 0..2 {
            now += TICK;
            reserve -= pacer.release_allowance(now, reserve);
        }
    }
    assert!(reserve > 0, "a lead should be held back for a stall");

    // The measured 410ms stall from grok-4.5, with nothing arriving.
    let mut moved = 0;
    for _ in 0..(410 / 24) {
        now += TICK;
        let allowance = pacer.release_allowance(now, reserve);
        reserve -= allowance;
        if allowance > 0 {
            moved += 1;
        }
    }
    assert!(
        moved > 0,
        "the reserve should keep text moving through a stall"
    );
    // Proportional drain empties over roughly TARGET_LEAD once arrivals stop.
    assert!(
        reserve < 3,
        "stall longer than {TARGET_LEAD:?} should nearly empty the reserve, left {reserve}"
    );
}

#[test]
fn releases_a_whole_response_without_pacing_it() {
    let mut pacer = StreamPacer::default();
    let now = Instant::now();
    let reserve = MAX_RESERVE_CHARS + 1;
    pacer.note_refill(now);

    assert_eq!(
        pacer.release_allowance(now + TICK, reserve),
        reserve,
        "text that arrived whole must not type itself out"
    );
}

#[test]
fn a_new_burst_cannot_spend_time_when_the_reserve_was_empty() {
    let mut pacer = StreamPacer::default();
    let start = Instant::now();

    pacer.note_refill(start);
    assert_eq!(pacer.release_allowance(start, 5), 0);

    let next_arrival = start + Duration::from_secs(1);
    pacer.note_refill(next_arrival);
    assert_eq!(
        pacer.release_allowance(next_arrival, 5),
        0,
        "idle time must not release text that only just arrived"
    );
    assert!(
        pacer.release_allowance(next_arrival + TICK, 5) > 0,
        "the new reserve should move on the following pacing tick"
    );
}

#[test]
fn releases_nothing_when_no_text_is_held() {
    let mut pacer = StreamPacer::default();
    let now = Instant::now();

    assert_eq!(pacer.release_allowance(now + TICK, 0), 0);
}

#[test]
fn partial_preview_tick_does_not_reschedule_after_hold_drains() {
    let mut streams = StreamUi::default();
    let start = Instant::now();
    streams.current_stream_kind = Some(StreamKind::Assistant);

    streams.push_delta(StreamKind::Assistant, "a", start);
    // Enough elapsed that the next arrival empties the hold into pending.
    let after_lead = start + Duration::from_millis(200);
    streams.push_delta(StreamKind::Assistant, "b", after_lead);
    assert!(streams.hold.is_empty());
    assert!(
        streams
            .stream(StreamKind::Assistant)
            .pending_text()
            .chars()
            .count()
            >= 2
    );
    assert!(
        streams.stream_tick_deadline.is_some(),
        "partial pending text should schedule one preview tick"
    );

    // No held text to release; this tick exists only for preview refresh.
    assert!(!streams.on_tick(after_lead + STREAM_UI_TICK));
    assert!(
        streams.stream_tick_deadline.is_none(),
        "static partial preview must not keep the 24ms cadence alive"
    );

    // Later ticks with no new arrivals must stay idle.
    assert!(!streams.on_tick(after_lead + STREAM_UI_TICK * 2));
    assert!(streams.stream_tick_deadline.is_none());
}

#[test]
fn held_text_keeps_rescheduling_until_the_reserve_drains() {
    let mut streams = StreamUi::default();
    let start = Instant::now();
    streams.current_stream_kind = Some(StreamKind::Assistant);

    // Small first char starts the clock without releasing; a later burst builds
    // a reserve that needs multiple ticks to empty.
    streams.push_delta(StreamKind::Assistant, "x", start);
    streams.push_delta(StreamKind::Assistant, &"y".repeat(40), start + TICK);
    assert!(!streams.hold.is_empty());

    let mut now = streams
        .stream_tick_deadline
        .expect("held text should schedule pacing ticks");
    let mut saw_reschedule = false;
    for _ in 0..30 {
        let _released = streams.on_tick(now);
        if streams.hold.is_empty() {
            assert!(
                streams.stream_tick_deadline.is_none(),
                "once the hold is empty, ticks must stop"
            );
            break;
        }
        let deadline = streams
            .stream_tick_deadline
            .expect("paced hold should reschedule while draining");
        saw_reschedule = true;
        now = deadline;
    }
    assert!(
        saw_reschedule,
        "paced hold should reschedule while draining"
    );
    assert!(streams.hold.is_empty());
}
