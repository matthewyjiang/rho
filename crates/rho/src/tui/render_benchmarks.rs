//! Allocation and latency benchmarks for the truncation hot path.
//!
//! `truncate_one_line` and `truncate_keep_end` are called for every visible
//! picker row, badge, preview, footer, and status line on each redraw. The
//! common case is a label with no newlines that fits the width. The optimized
//! fast path skips the unconditional `replace('\n', " ")` copy the old code did
//! on every call, and the truncation path collapses two allocations into one.
//!
//! Run with:
//!   CARGO_PROFILE_TEST_OPT_LEVEL=3 cargo test -p rho-coding-agent --lib \
//!     'tui::render::render_benchmarks::' -- --ignored --nocapture --test-threads=1
//!
//! The global allocator is shared across tests, so use `--test-threads=1`
//! to keep allocation counts stable when running all benchmarks together.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use super::*;

struct CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() && new_size > layout.size() {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Count allocations and bytes during one operation block.
fn alloc_count(operation: impl FnOnce()) -> (usize, usize) {
    // Reset is racy across threads but these benchmarks run single-threaded
    // with --nocapture, and the comparison benchmark controls its own noise by
    // measuring old and new back-to-back under the same conditions.
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    operation();
    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

fn median_ns(samples: usize, mut operation: impl FnMut()) -> u64 {
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        operation();
        durations.push(start.elapsed().as_nanos() as u64);
    }
    durations.sort_unstable();
    durations[durations.len() / 2]
}

const SAMPLES: usize = 2000;
const WARMUP: usize = 200;

fn warmup(mut operation: impl FnMut()) {
    for _ in 0..WARMUP {
        operation();
    }
}

/// The old `truncate_one_line` implementation, kept inline for before/after
/// comparison. The old code unconditionally called `replace('\n', " ")`,
/// allocating a new String even when the text had no newlines and fit the width.
fn old_truncate_one_line(text: &str, width: usize) -> String {
    let text = text.replace('\n', " ");
    if UnicodeWidthStr::width(text.as_str()) <= width {
        return text;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    truncate_to_display_width(&text, width - 1).into_owned() + "…"
}

/// The old `truncate_keep_end` implementation, kept inline for comparison.
fn old_truncate_keep_end(text: &str, width: usize) -> String {
    let text = text.replace('\n', " ");
    if width == 0 {
        return String::new();
    }
    if display_width(&text) <= width {
        return text;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let target = width - 1;
    let mut start = text.len();
    let mut used = 0usize;
    for (index, ch) in text.char_indices().rev() {
        let ch_width = char_display_width(ch);
        if used + ch_width > target {
            break;
        }
        used += ch_width;
        start = index;
    }
    format!("…{}", &text[start..])
}

/// Print a before/after comparison line and assert the new path is no worse.
fn compare(
    name: &str,
    old_fn: impl Fn(&str, usize) -> String,
    new_fn: impl Fn(&str, usize) -> String,
    text: &str,
    width: usize,
) {
    warmup(|| {
        let _ = old_fn(text, width);
    });
    let (old_count, old_bytes) = alloc_count(|| {
        for _ in 0..1000 {
            black_box(old_fn(text, width));
        }
    });
    let old_ns = median_ns(SAMPLES, || {
        old_fn(text, width);
    });

    warmup(|| {
        let _ = new_fn(text, width);
    });
    let (new_count, new_bytes) = alloc_count(|| {
        for _ in 0..1000 {
            black_box(new_fn(text, width));
        }
    });
    let new_ns = median_ns(SAMPLES, || {
        new_fn(text, width);
    });

    eprintln!(
        "{name} - OLD: {old} allocs/call, {old_b} bytes, {old_ns} ns | \
         NEW: {new} allocs/call, {new_b} bytes, {new_ns} ns | \
         allocs {alloc_improve:.0}%, bytes {byte_improve:.0}%, latency {lat_improve:.0}%",
        old = old_count as f64 / 1000.0,
        new = new_count as f64 / 1000.0,
        old_b = old_bytes,
        new_b = new_bytes,
        alloc_improve = (1.0 - new_count as f64 / old_count.max(1) as f64) * 100.0,
        byte_improve = (1.0 - new_bytes as f64 / old_bytes.max(1) as f64) * 100.0,
        lat_improve = (1.0 - new_ns as f64 / old_ns.max(1) as f64) * 100.0,
    );

    // The new path must not allocate more bytes than the old path.
    assert!(
        new_bytes <= old_bytes,
        "{name}: regression: new {new_bytes} bytes > old {old_bytes} bytes"
    );
    // The new path must not be materially slower.
    assert!(
        new_ns <= old_ns + 100,
        "{name}: latency regression: new {new_ns} ns > old {old_ns} ns + 100"
    );
}

#[test]
#[ignore = "allocation benchmark; run with --ignored --nocapture"]
fn truncate_one_line_fitting_label_comparison() {
    // Common picker case: short label, no newlines, fits the width.
    // Old: replace (1 alloc) + return that String (0 extra) = 1 alloc.
    // New: to_string (1 alloc). The width scan is allocation-free.
    // Both do 1 alloc here, but the new path avoids the replace overhead.
    compare(
        "truncate_one_line fit",
        old_truncate_one_line,
        truncate_one_line,
        "claude-sonnet-4-5",
        80,
    );
}

#[test]
#[ignore = "allocation benchmark; run with --ignored --nocapture"]
fn truncate_one_line_truncation_comparison() {
    // Truncation case: label longer than width.
    // Old: replace (1) + into_owned() (1) + "…" concat (1) = 3 allocs, 2 full copies.
    // New: skip replace, format!("{}…", truncate_to_display_width(...)) = 1 alloc.
    compare(
        "truncate_one_line truncate",
        old_truncate_one_line,
        truncate_one_line,
        "claude-sonnet-4-5-20250514-very-long-suffix-that-exceeds-width",
        20,
    );
}

#[test]
#[ignore = "allocation benchmark; run with --ignored --nocapture"]
fn truncate_one_line_newline_comparison() {
    // Newline case: must normalize. Both paths do replace.
    compare(
        "truncate_one_line newline",
        old_truncate_one_line,
        truncate_one_line,
        "first line\nsecond line\nthird",
        80,
    );
}

#[test]
#[ignore = "allocation benchmark; run with --ignored --nocapture"]
fn truncate_keep_end_fitting_comparison() {
    // Statusline path display that fits.
    compare(
        "truncate_keep_end fit",
        old_truncate_keep_end,
        truncate_keep_end,
        "/home/user/projects/rho/crates/rho/src/tui/render.rs",
        120,
    );
}

#[test]
#[ignore = "allocation benchmark; run with --ignored --nocapture"]
fn truncate_keep_end_truncated_comparison() {
    // Long path truncated from the front.
    compare(
        "truncate_keep_end truncate",
        old_truncate_keep_end,
        truncate_keep_end,
        "/home/user/projects/rho/crates/rho/src/tui/render.rs/subdir/deeper",
        20,
    );
}
