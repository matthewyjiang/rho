//! Ignored optimized-test instrumentation for interactive TUI render hot paths.
//!
//! Fixtures are built outside timed samples. Measured call sites keep exact
//! production behavior; this module does not change public APIs.
//!
//! Scenarios:
//! - full frame draw at bottom-follow over geometric transcript sizes
//! - mouse-move hover handling on a long transcript
//! - wheel scrolling on a long transcript
//! - streaming commit ticks appending to one assistant / reasoning entry
//!
//! Run with:
//!   CARGO_PROFILE_TEST_OPT_LEVEL=3 cargo test -p rho-coding-agent --lib \
//!     tui::performance_benchmarks -- --ignored --nocapture

use std::{hint::black_box, time::Instant};

use crossterm::event::MouseEventKind;
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value};

use super::{tests::test_app, App, Entry, ReasoningEntry, ToolEntry};

/// Geometric transcript sizes expose non-linear frame-cost regressions.
const TRANSCRIPT_SIZES: [usize; 3] = [250, 1_000, 4_000];
/// Streamed commit counts for one growing entry; growth should stay near-linear.
const STREAM_COMMIT_SIZES: [usize; 2] = [250, 1_000];
/// 4x transcript growth must not move warm frame cost by more than this factor.
///
/// Warm frames read only the visible window plus O(log n) range lookups, so
/// the honest expectation is "flat". The allowance absorbs allocator and cache
/// noise, not real per-entry work.
const MAX_FRAME_GROWTH_RATIO: f64 = 2.0;
/// 4x streamed-commit growth should stay near-linear in total time; catch
/// quadratic re-render of the growing entry.
const MAX_NORMALIZED_STREAM_GROWTH: f64 = 2.0;
const WARMUP_ITERS: usize = 3;
const TERMINAL_WIDTH: u16 = 120;
const TERMINAL_HEIGHT: u16 = 40;

#[test]
#[ignore = "optimized TUI render hot-path benchmark; run with CARGO_PROFILE_TEST_OPT_LEVEL=3"]
fn run_render_hot_path_benchmarks() {
    let samples = std::env::var("RHO_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30)
        .max(5);
    // Fixed scheme: matches real sessions, where style lookups resolve against
    // an actual palette instead of the bare no-sample terminal fallback.
    let _theme = super::theme::theme_test_lock();
    super::theme::Theme::apply_committed("one-half-dark");
    let mut check_failures = Vec::new();

    let mut frame_measurements = Vec::new();
    let mut previous_frame: Option<(usize, u64)> = None;
    for &entry_count in &TRANSCRIPT_SIZES {
        let (mut app, mut terminal) = transcript_fixture(entry_count);
        for _ in 0..WARMUP_ITERS {
            terminal.draw(|frame| app.draw(frame)).expect("warmup draw");
        }
        let timing = measure(samples, || {
            terminal.draw(|frame| app.draw(frame)).expect("timed draw");
        });
        if let Some((prev_count, prev_median)) = previous_frame {
            let ratio = timing.median() as f64 / prev_median.max(1) as f64;
            if ratio >= MAX_FRAME_GROWTH_RATIO {
                check_failures.push(format!(
                    "warm frame cost grew with transcript length: {prev_count}->{entry_count} \
                     entries, time ratio {ratio:.2} (max {MAX_FRAME_GROWTH_RATIO})"
                ));
            }
        }
        previous_frame = Some((entry_count, timing.median()));
        frame_measurements.push(json!({
            "entry_count": entry_count,
            "timing": timing.json(),
        }));
    }

    let (mut app, mut terminal) = transcript_fixture(*TRANSCRIPT_SIZES.last().expect("sizes"));
    terminal.draw(|frame| app.draw(frame)).expect("warm draw");
    // Alternate positions: identical consecutive positions short-circuit.
    let mut flip = false;
    let mouse_move_timing = measure(samples, || {
        flip = !flip;
        let row = if flip { 10 } else { 11 };
        app.handle_mouse_event(MouseEventKind::Moved, 20, row, &mut terminal)
            .expect("timed mouse move")
    });

    let scroll_timing = measure(samples, || {
        flip = !flip;
        let kind = if flip {
            MouseEventKind::ScrollUp
        } else {
            MouseEventKind::ScrollDown
        };
        app.handle_mouse_event(kind, 20, 10, &mut terminal)
            .expect("timed wheel scroll")
    });

    let full_rebuild_timing = measure(samples, || {
        app.history.invalidate_from(0);
        let area = ratatui::layout::Rect::new(0, 0, TERMINAL_WIDTH, TERMINAL_HEIGHT);
        black_box(app.frame_context(area).history_len)
    });

    let assistant_stream =
        stream_commit_measurements(Entry::Assistant, "assistant", &mut check_failures);
    let reasoning_stream = stream_commit_measurements(
        |chunk| Entry::Reasoning(ReasoningEntry::new(chunk)),
        "reasoning",
        &mut check_failures,
    );

    let report = json!({
        "schema_version": 1,
        "suite": "rho-tui-render-hot-path-benchmarks",
        "profile": "test with opt-level=3",
        "sample_count": samples,
        "candidate_commit": command_output("git", &["rev-parse", "--short", "HEAD"]),
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "terminal": { "width": TERMINAL_WIDTH, "height": TERMINAL_HEIGHT },
        "checks": {
            "frame_max_growth_ratio": MAX_FRAME_GROWTH_RATIO,
            "stream_max_normalized_growth": MAX_NORMALIZED_STREAM_GROWTH,
            "transcript_sizes": TRANSCRIPT_SIZES,
            "stream_commit_sizes": STREAM_COMMIT_SIZES,
        },
        "measurements": {
            "warm_frame_draw_sizes": frame_measurements,
            "mouse_move_hover": mouse_move_timing.json(),
            "wheel_scroll": scroll_timing.json(),
            "full_transcript_rebuild": full_rebuild_timing.json(),
            "assistant_stream_commits": assistant_stream,
            "reasoning_stream_commits": reasoning_stream,
        },
    });

    let rendered = serde_json::to_string_pretty(&report).expect("serialize benchmark report");
    println!("{rendered}");
    if let Some(path) = std::env::var_os("RHO_BENCH_OUTPUT") {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).expect("create RHO_BENCH_OUTPUT parent");
            }
        }
        std::fs::write(&path, format!("{rendered}\n")).expect("write RHO_BENCH_OUTPUT");
    }
    assert!(
        check_failures.is_empty(),
        "growth checks failed:\n{}",
        check_failures.join("\n")
    );
}

/// Total time to stream `commit_count` fragments into one growing entry,
/// following the production commit path: append the fragment, then refresh the
/// cached line count exactly as the next frame would.
fn stream_commit_measurements(
    entry: impl Fn(String) -> Entry,
    label: &str,
    check_failures: &mut Vec<String>,
) -> Value {
    let mut measurements = Vec::new();
    let mut previous: Option<(usize, u64)> = None;
    for &commit_count in &STREAM_COMMIT_SIZES {
        let started = Instant::now();
        let mut app = test_app();
        let area = ratatui::layout::Rect::new(0, 0, TERMINAL_WIDTH, TERMINAL_HEIGHT);
        black_box(app.frame_context(area).history_len);
        for index in 0..commit_count {
            app.push_transcript_entry(entry(stream_chunk(index)));
            black_box(app.frame_context(area).history_len);
        }
        let elapsed = started.elapsed().as_nanos() as u64;
        if let Some((prev_count, prev_elapsed)) = previous {
            let size_ratio = commit_count as f64 / prev_count as f64;
            let time_ratio = elapsed as f64 / prev_elapsed.max(1) as f64;
            let normalized = time_ratio / size_ratio;
            if normalized >= MAX_NORMALIZED_STREAM_GROWTH {
                check_failures.push(format!(
                    "{label} stream commit growth regressed: {prev_count}->{commit_count} \
                     commits, time ratio {time_ratio:.2} over size ratio {size_ratio:.2} \
                     (normalized {normalized:.2}, max {MAX_NORMALIZED_STREAM_GROWTH})"
                ));
            }
        }
        previous = Some((commit_count, elapsed));
        measurements.push(json!({
            "commit_count": commit_count,
            "total_ns": elapsed,
            "per_commit_ns": elapsed / commit_count as u64,
        }));
    }
    json!(measurements)
}

fn stream_chunk(index: usize) -> String {
    // Mostly prose with periodic hard newlines so commits wrap and close lines.
    if index % 8 == 7 {
        format!("chunk {index} closes this paragraph.\n\n")
    } else {
        format!("chunk {index} adds a handful of words to the open paragraph, ")
    }
}

fn transcript_fixture(entry_count: usize) -> (App, Terminal<TestBackend>) {
    let mut app = test_app();
    for index in 0..entry_count {
        app.push_transcript_entry(match index % 4 {
            0 => Entry::User(format!(
                "user prompt {index}: please look at module {index}"
            )),
            1 => Entry::Assistant(assistant_markdown(index)),
            2 => bench_tool_entry(index),
            _ => Entry::Reasoning(ReasoningEntry::new(format!(
                "considering approach {index} against the alternatives before answering"
            ))),
        });
    }
    let terminal =
        Terminal::new(TestBackend::new(TERMINAL_WIDTH, TERMINAL_HEIGHT)).expect("test terminal");
    (app, terminal)
}

fn assistant_markdown(index: usize) -> String {
    format!(
        "Here is finding {index} with some `inline code` and a short list:\n\
         - first point about the change\n\
         - second point with more detail\n\n\
         ```rust\n\
         fn example_{index}() -> usize {{\n\
             let value = {index};\n\
             value * 2\n\
         }}\n\
         ```\n\
         Closing paragraph explaining what the snippet above demonstrates."
    )
}

fn bench_tool_entry(index: usize) -> Entry {
    let card = rho_tools::tool_card::ToolCard::new(
        rho_tools::tool_card::ToolStatus::Ok,
        rho_tools::tool_card::ToolFamily::Default,
        rho_tools::tool_card::ToolHeader::call(format!("bench tool {index}"), None),
    )
    .with_body(rho_tools::tool_card::ToolBody::Lines(vec![
        format!("output line one for call {index}"),
        format!("output line two for call {index}"),
        format!("output line three for call {index}"),
    ]));
    Entry::Tool(ToolEntry::new(card, false, None, None))
}

struct SampleStats {
    samples_ns: Vec<u64>,
}

impl SampleStats {
    fn new(mut samples_ns: Vec<u64>) -> Self {
        samples_ns.sort_unstable();
        Self { samples_ns }
    }

    fn percentile(&self, percentile: usize) -> u64 {
        let index = ((self.samples_ns.len() - 1) * percentile).div_ceil(100);
        self.samples_ns[index]
    }

    fn median(&self) -> u64 {
        self.percentile(50)
    }

    fn json(&self) -> Value {
        json!({
            "unit": "nanoseconds",
            "median": self.median(),
            "p95": self.percentile(95),
            "p99": self.percentile(99),
        })
    }
}

fn measure<T>(samples: usize, mut operation: impl FnMut() -> T) -> SampleStats {
    let durations = (0..samples)
        .map(|_| {
            let started = Instant::now();
            black_box(operation());
            started.elapsed().as_nanos() as u64
        })
        .collect();
    SampleStats::new(durations)
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    std::process::Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}
