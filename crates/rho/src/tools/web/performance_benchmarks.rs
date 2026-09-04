//! Opt-in measurements through the actual get_search_content selection path.
//! Baseline-copy instructions in docs/performance-audit.md apply only to the
//! recorded audit revisions.

use std::{hint::black_box, time::Instant};

use serde_json::json;

use super::{
    adapters::{GetSearchContent, GetSearchContentArgs},
    storage::{new_response_id, StoredContent, StoredItem, WebAccessStore},
};

#[test]
#[ignore = "performance measurement; run with CARGO_PROFILE_TEST_OPT_LEVEL=3"]
fn perf_audit_web_content_selection() {
    let samples = std::env::var("RHO_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20)
        .max(1);
    // Geometric sizes expose copying of unselected bodies. All fit the cache.
    for sibling_bytes in [256 * 1024, 1024 * 1024, 8 * 1024 * 1024] {
        let root = tempfile::tempdir().expect("web store root");
        let store = WebAccessStore::with_root(root.path().to_path_buf());
        let response_id = new_response_id();
        let item = |content: String| StoredItem {
            url: None,
            query: None,
            title: None,
            content,
            metadata: json!({}),
        };
        store
            .store(
                response_id.clone(),
                StoredContent {
                    kind: "fetch_content".into(),
                    items: vec![
                        item("selected body".into()),
                        item("x".repeat(sibling_bytes)),
                    ],
                },
            )
            .expect("seed store");
        let tool = GetSearchContent::new(store);
        let read = || {
            tool.execute(
                GetSearchContentArgs {
                    response_id: response_id.clone(),
                    query: None,
                    query_index: None,
                    url: None,
                    url_index: Some(0),
                },
                /*max_output_bytes*/ 1024,
                String::new(),
            )
            .expect("select stored item")
        };
        let expected = read();
        let mut timings = Vec::with_capacity(samples);
        // Batch short cache hits so timer overhead does not dominate samples.
        const ITERATIONS: u32 = 100;
        for _ in 0..samples {
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                black_box(read());
            }
            timings.push(start.elapsed().as_nanos());
        }
        assert_eq!(read().content, expected.content);
        println!(
            "{}",
            json!({
                "scenario": "web_content_selection",
                "sibling_bytes": sibling_bytes,
                "iterations_per_sample": ITERATIONS,
                "samples_ns": timings,
            })
        );
    }
}
