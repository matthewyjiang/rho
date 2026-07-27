use std::time::{Duration, Instant};

use super::{StreamPacer, MAX_RESERVE_CHARS};

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
        reserve += burst;
        pacer.record_arrival(now, burst, reserve == burst);
        for _ in 0..ticks.max(1) {
            now += TICK;
            let allowance = pacer.release_allowance(now, reserve);
            reserve -= allowance;
            // The very first tick has no interval to bill against.
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

    // Establish a rate, then go quiet for far longer than a normal flush.
    let mut reserve = 0usize;
    for _ in 0..20 {
        reserve += 13;
        pacer.record_arrival(now, 13, reserve == 13);
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
}

#[test]
fn releases_a_whole_response_without_pacing_it() {
    let mut pacer = StreamPacer::default();
    let now = Instant::now();
    let reserve = MAX_RESERVE_CHARS + 1;
    pacer.record_arrival(now, reserve, true);

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

    pacer.record_arrival(start, 5, true);
    assert_eq!(pacer.release_allowance(start, 5), 0);

    let next_arrival = start + Duration::from_secs(1);
    pacer.record_arrival(next_arrival, 5, true);
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
