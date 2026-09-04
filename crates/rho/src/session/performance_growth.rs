//! Baseline-compatible growth instrumentation. Copy this file and its module
//! declaration in performance_benchmarks.rs unchanged to 06d1ff28.

use std::{fs, hint::black_box, time::Instant};

use rho_providers::model::Message;
use serde_json::json;

use super::{snapshot, SUMMARIZE_SIZES};
use crate::session::{
    persistence::{summarize_session_file, SessionEntry, StoredDisplayMessage, SESSION_VERSION},
    snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta},
    tree::{NodeId, SessionNode, SessionNodeKind, SessionTree, StoredStateTransition},
    Session,
};

// Performance evidence, not a timing assertion: growing histories expose prefix
// copying hidden by the fixed-history benchmark. Session persistence owns this.
#[test]
#[ignore = "optimized growth benchmark; run with CARGO_PROFILE_TEST_OPT_LEVEL=3"]
fn perf_audit_session_growing_history() {
    let samples: usize = std::env::var("RHO_BENCH_SAMPLES")
        .map(|value| value.parse().expect("RHO_BENCH_SAMPLES must be an integer"))
        .unwrap_or(7);
    assert!(samples > 0, "RHO_BENCH_SAMPLES must be positive");

    for nodes in SUMMARIZE_SIZES {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
        let mut contents = String::new();
        let header = SessionEntry::Session {
            version: SESSION_VERSION,
            id: session.id().to_owned(),
            timestamp: "1700000000".into(),
            cwd: cwd.path().to_owned(),
            agent_id: None,
            agent_fingerprint: None,
        };
        push_entry(&mut contents, &header);
        let mut history = Vec::new();
        let mut previous = None;
        let mut parent_id = None;
        for index in 0..nodes {
            let tail = turn(index);
            history.extend(tail.clone());
            let current = snapshot(
                &session,
                index as u64 + 1,
                history.clone(),
                Default::default(),
            );
            let transition = match &previous {
                None => StoredStateTransition::Snapshot {
                    snapshot: Box::new(current.clone()),
                },
                Some(previous) => StoredStateTransition::SnapshotDelta {
                    delta: Box::new(
                        StoredSnapshotDelta::after(
                            &SnapshotDeltaBase::from_snapshot(previous),
                            &current,
                        )
                        .unwrap(),
                    ),
                },
            };
            let id = NodeId::from_string(format!("growth-{index}")).unwrap();
            push_entry(
                &mut contents,
                &SessionEntry::Node {
                    node: SessionNode {
                        id: id.clone(),
                        parent_id,
                        timestamp: "1700000000".into(),
                        kind: SessionNodeKind::Commit,
                        compaction_facts: None,
                        transition,
                        display_messages: tail
                            .into_iter()
                            .map(|message| StoredDisplayMessage {
                                timestamp: "1700000000".into(),
                                message,
                            })
                            .collect(),
                    },
                },
            );
            parent_id = Some(id);
            previous = Some(current);
        }
        fs::write(session.path(), &contents).unwrap();
        let bytes = contents.len();
        drop(contents);
        let expected = previous.unwrap();
        // Validation and filesystem/cache warmup stay outside the timed region.
        let tree = SessionTree::load(session.path()).unwrap();
        pretty_assertions::assert_eq!(
            tree.active_state().unwrap().snapshot.as_ref(),
            Some(&expected)
        );
        session.cache_loaded_tree(tree);
        black_box(summarize_session_file(session.path(), cwd.path()).unwrap());

        for scenario in ["session_tree_load_growing", "session_summarize_growing"] {
            let mut samples_ns = Vec::with_capacity(samples);
            for _ in 0..samples {
                let started = Instant::now();
                match scenario {
                    "session_tree_load_growing" => {
                        black_box(SessionTree::load(session.path()).unwrap());
                    }
                    "session_summarize_growing" => {
                        black_box(summarize_session_file(session.path(), cwd.path()).unwrap());
                    }
                    _ => unreachable!(),
                }
                samples_ns.push(started.elapsed().as_nanos() as u64);
            }
            println!(
                "{}",
                json!({"scenario": scenario, "nodes": nodes,
                "history_messages": history.len(), "bytes": bytes, "samples_ns": samples_ns})
            );
        }

        // Warm the SQLite index and append cache before measuring saves. Each
        // measured save adds one real turn; snapshot construction is excluded.
        let mut samples_ns = Vec::with_capacity(samples);
        for sample in 0..=samples {
            let tail = turn(nodes + sample);
            history.extend(tail.clone());
            let current = snapshot(
                &session,
                (nodes + sample + 1) as u64,
                history.clone(),
                Default::default(),
            );
            let started = Instant::now();
            session.save_snapshot(&current, &tail).unwrap();
            let elapsed = started.elapsed().as_nanos() as u64;
            if sample > 0 {
                samples_ns.push(elapsed);
            }
        }
        println!(
            "{}",
            json!({"scenario": "session_save_growing", "nodes_before_warmup": nodes,
            "history_messages_first_sample": (nodes + 2) * 2,
            "history_messages_last_sample": history.len(), "samples_ns": samples_ns})
        );
        let loaded = SessionTree::load(session.path()).unwrap();
        pretty_assertions::assert_eq!(&loaded.active_state().unwrap().model, &history);
    }
}

fn push_entry(contents: &mut String, entry: &SessionEntry) {
    contents.push_str(&serde_json::to_string(entry).unwrap());
    contents.push('\n');
}

fn turn(index: usize) -> Vec<Message> {
    // A small user request plus a 1 KiB assistant body, reported by fixture size.
    vec![
        Message::user_text(format!("turn {index}")),
        Message::assistant_text("x".repeat(1024)),
    ]
}
