//! TUI scaling measurements. Baseline-copy instructions in
//! docs/performance-audit.md apply only to the recorded audit revisions.
//! Fixtures and incoming fragments are prepared before each timed sample.
//! Advisory measurements only: no machine-dependent timing assertions.

use std::{hint::black_box, time::Instant};

use ratatui::{layout::Rect, style::Style, text::Span};
use serde_json::json;

use crate::tui::{
    history_cache::{HistoryLineCache, HistoryRenderSettings},
    render::hard_wrap_styled_spans,
    tests::test_app,
    Entry,
};

// Fourfold growth separates linear work from repeated prefix walks. Seven
// independent fixtures give a median without retaining all fixtures in memory.
const SIZES: [usize; 3] = [256, 1_024, 4_096];
const SAMPLES: usize = 7;

fn report(case: &str, size: usize, operations: usize, mut ns: Vec<u64>) {
    ns.sort_unstable();
    println!(
        "{}",
        json!({
            "suite": "tui_perf_audit", "scenario": case, "size": size,
            "operations": operations, "samples_ns": ns,
            "median_ns": ns[ns.len() / 2],
        })
    );
}

#[test]
#[ignore = "advisory optimized benchmark; run with opt-level=3 and --nocapture"]
fn perf_audit_fence_stream_commits() {
    let _theme = crate::tui::theme::theme_test_lock();
    crate::tui::theme::Theme::apply_committed("one-half-dark");
    for size in SIZES {
        let mut ns = Vec::new();
        for _ in 0..SAMPLES {
            let mut app = test_app();
            let area = Rect::new(0, 0, 120, 40);
            // Plain code isolates the incremental cache from syntax grammar cost.
            app.push_transcript_entry(Entry::Assistant("```text\n".into()));
            black_box(app.frame_context(area).history_len);
            let fragments: Vec<_> = (0..size)
                .map(|index| {
                    Entry::Assistant(format!("value_{index} = λ🙂; plain code body\n").into())
                })
                .collect();
            let started = Instant::now();
            for fragment in fragments {
                app.push_transcript_entry(fragment);
                black_box(app.frame_context(area).history_len);
            }
            ns.push(started.elapsed().as_nanos() as u64);
            // Closing is intentionally outside the timed open-stream sample.
            app.push_transcript_entry(Entry::Assistant("```".into()));
            black_box(app.frame_context(area).history_len);
        }
        report("fence_stream_commits", size, size, ns);
    }
}

#[test]
#[ignore = "advisory optimized benchmark; run with opt-level=3 and --nocapture"]
fn perf_audit_tail_updates_by_history_size() {
    let _theme = crate::tui::theme::theme_test_lock();
    // Fixed update work at each history size isolates range-maintenance cost.
    const UPDATES: usize = 128;
    let settings = HistoryRenderSettings {
        width: 120,
        max_tool_output_lines: 10,
        zen_mode: false,
        theme_generation: 0,
        max_image_height: crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
    };
    for size in SIZES {
        let mut ns = Vec::new();
        for _ in 0..SAMPLES {
            let mut entries: Vec<_> = (0..size)
                .map(|index| Entry::User(format!("history entry {index}")))
                .collect();
            entries.push(Entry::Assistant("```text\n".into()));
            let mut cache = HistoryLineCache::default();
            black_box(cache.line_count(&entries, settings, &|_, _| Vec::new()));
            let started = Instant::now();
            for _ in 0..UPDATES {
                let Entry::Assistant(tail) = &mut entries[size] else {
                    unreachable!();
                };
                tail.push_str("appended code line\n");
                cache.entry_appended(size);
                black_box(cache.line_count(&entries, settings, &|_, _| Vec::new()));
            }
            ns.push(started.elapsed().as_nanos() as u64);
        }
        report("tail_updates_by_history_size", size, UPDATES, ns);
    }
}

#[test]
#[ignore = "advisory optimized benchmark; run with opt-level=3 and --nocapture"]
fn perf_audit_styled_wraps() {
    for size in SIZES {
        let spans: Vec<_> = (0..size)
            .map(|index| {
                Span::styled(
                    "word_λ界🙂 ",
                    Style::default().fg(ratatui::style::Color::Indexed((index % 16) as u8)),
                )
            })
            .collect();
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        black_box(hard_wrap_styled_spans(&text, &spans, 80, Style::default()));
        let ns = (0..SAMPLES)
            .map(|_| {
                let started = Instant::now();
                black_box(hard_wrap_styled_spans(&text, &spans, 80, Style::default()));
                started.elapsed().as_nanos() as u64
            })
            .collect();
        report("styled_wraps", size, 1, ns);
    }
}
