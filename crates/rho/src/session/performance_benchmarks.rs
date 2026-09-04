//! Ignored optimized-test instrumentation for private session hot paths.
//!
//! Fixtures are built outside timed samples. Measured call sites keep exact
//! production behavior; this module does not change public APIs.
//!
//! Scenarios:
//! - list cold / stale / warm index paths
//! - summarize geometric transcript sizes with growth-ratio checks

use std::{
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use rho_providers::model::{Message, ModelIdentity};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::persistence::{
    session_dir_in_root, summarize_session_file, SessionEntry, SESSION_TRANSCRIPT_FILE_NAME,
    SESSION_VERSION,
};
use super::snapshot_delta::{SnapshotDeltaBase, StoredSnapshotDelta};
use super::tree::{NodeId, SessionNode, SessionNodeKind, StoredStateTransition};
use super::Session;

#[path = "performance_growth.rs"]
mod growth;

/// Practical fixed workspace size for list / index sync measurements.
const LIST_SESSION_COUNT: usize = 750;
/// Geometric transcript sizes expose non-linear summarize regressions.
const SUMMARIZE_SIZES: [usize; 3] = [250, 1_000, 4_000];
/// 4x entry growth should stay near-linear; catch quadratic-ish regressions.
const MAX_NORMALIZED_SIZE_GROWTH: f64 = 2.0;
const WARMUP_ITERS: usize = 3;

#[test]
#[ignore = "optimized session hot-path benchmark; run with CARGO_PROFILE_TEST_OPT_LEVEL=3"]
fn run_hot_path_benchmarks() {
    let samples = std::env::var("RHO_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20)
        .max(5);

    let list_fixture = ListFixture::build(LIST_SESSION_COUNT);

    // Cold: empty index, first list must discover and index every transcript.
    let cold_timing = measure(samples, || {
        // Drop cached connections before unlinking so the next open is a cold file.
        super::index::clear_index_connection_cache_for_test();
        let _ = fs::remove_file(list_fixture.root.path().join("index.sqlite3"));
        Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
            .expect("timed list_in_root cold")
    });
    assert_eq!(
        Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
            .expect("post-cold list")
            .len(),
        list_fixture.session_count
    );

    // Warm: index already current.
    for _ in 0..WARMUP_ITERS {
        black_box(
            Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
                .expect("warmup list_in_root warm"),
        );
    }
    let warm_timing = measure(samples, || {
        Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
            .expect("timed list_in_root warm")
    });

    // Stale: mutate every transcript so sync must re-summarize the batch.
    let stale_timing = measure(samples, || {
        list_fixture.touch_all_sessions();
        Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
            .expect("timed list_in_root stale")
    });

    let mut summarize_measurements = Vec::new();
    let mut previous: Option<(usize, u64)> = None;
    for &entry_count in &SUMMARIZE_SIZES {
        let fixture = SummarizeFixture::build(entry_count);
        for _ in 0..WARMUP_ITERS {
            black_box(
                summarize_session_file(&fixture.path, fixture.cwd.path())
                    .expect("warmup summarize_session_file"),
            );
        }
        let timing = measure(samples, || {
            summarize_session_file(&fixture.path, fixture.cwd.path())
                .expect("timed summarize_session_file")
        });
        if let Some((prev_count, prev_median)) = previous {
            let size_ratio = entry_count as f64 / prev_count as f64;
            let time_ratio = timing.median() as f64 / prev_median.max(1) as f64;
            let normalized = time_ratio / size_ratio;
            assert!(
                normalized < MAX_NORMALIZED_SIZE_GROWTH,
                "summarize growth regressed: {prev_count}->{entry_count} entries, \
                 time ratio {time_ratio:.2} over size ratio {size_ratio:.2} \
                 (normalized {normalized:.2}, max {MAX_NORMALIZED_SIZE_GROWTH})"
            );
        }
        previous = Some((entry_count, timing.median()));
        summarize_measurements.push(json!({
            "entry_count": entry_count,
            "bytes": fixture.bytes,
            "timing": timing.json(),
        }));
    }

    let report = json!({
        "schema_version": 2,
        "suite": "rho-session-hot-path-benchmarks",
        "profile": "test with opt-level=3",
        "sample_count": samples,
        "candidate_commit": command_output("git", &["rev-parse", "--short", "HEAD"]),
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "checks": {
            "summarize_max_normalized_growth": MAX_NORMALIZED_SIZE_GROWTH,
            "summarize_sizes": SUMMARIZE_SIZES,
        },
        "measurements": {
            "list_in_root_cold": {
                "session_count": list_fixture.session_count,
                "timing": cold_timing.json(),
            },
            "list_in_root_stale": {
                "session_count": list_fixture.session_count,
                "timing": stale_timing.json(),
            },
            "list_in_root_warm": {
                "session_count": list_fixture.session_count,
                "timing": warm_timing.json(),
            },
            "summarize_session_file_sizes": summarize_measurements,
        },
    });

    let rendered = serde_json::to_string_pretty(&report).expect("serialize benchmark report");
    println!("{rendered}");
    if let Some(path) = std::env::var_os("RHO_BENCH_OUTPUT") {
        if let Some(parent) = Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).expect("create RHO_BENCH_OUTPUT parent");
            }
        }
        fs::write(&path, format!("{rendered}\n")).expect("write RHO_BENCH_OUTPUT");
    }
}

struct ListFixture {
    root: TempDir,
    cwd: TempDir,
    session_count: usize,
    session_paths: Vec<PathBuf>,
}

impl ListFixture {
    fn build(session_count: usize) -> Self {
        let root = tempfile::tempdir().expect("session root");
        let cwd = tempfile::tempdir().expect("workspace cwd");
        let mut session_paths = Vec::with_capacity(session_count);
        for index in 0..session_count {
            session_paths.push(write_minimal_session(root.path(), cwd.path(), index));
        }
        Self {
            root,
            cwd,
            session_count,
            session_paths,
        }
    }

    fn touch_all_sessions(&self) {
        for path in &self.session_paths {
            let mut contents = fs::read_to_string(path).expect("read session for stale touch");
            // Append a blank line so size changes without breaking JSONL recovery.
            contents.push('\n');
            fs::write(path, contents).expect("touch session for stale sync");
        }
    }
}

struct SummarizeFixture {
    _root: TempDir,
    cwd: TempDir,
    path: PathBuf,
    bytes: usize,
}

impl SummarizeFixture {
    fn build(entry_count: usize) -> Self {
        let root = tempfile::tempdir().expect("summarize session root");
        let cwd = tempfile::tempdir().expect("summarize cwd");
        let workspace = session_dir_in_root(root.path(), cwd.path());
        fs::create_dir_all(&workspace).expect("summarize workspace dir");
        let session_dir = workspace.join(format!("1700000000_bench-large-{entry_count}"));
        fs::create_dir_all(&session_dir).expect("summarize session dir");
        let path = session_dir.join(SESSION_TRANSCRIPT_FILE_NAME);
        let bytes = write_large_transcript(&path, cwd.path(), entry_count);
        Self {
            _root: root,
            cwd,
            path,
            bytes,
        }
    }
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
            "samples": self.samples_ns,
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

fn write_minimal_session(session_root: &Path, cwd: &Path, index: usize) -> PathBuf {
    let id = format!("bench-session-{index:04x}");
    let created_at = 1_700_000_000 + index as u64;
    let workspace = session_dir_in_root(session_root, cwd);
    fs::create_dir_all(&workspace).expect("list workspace dir");
    let session_dir = workspace.join(format!("{created_at}_{id}"));
    fs::create_dir_all(&session_dir).expect("list session dir");
    let path = session_dir.join(SESSION_TRANSCRIPT_FILE_NAME);
    let header = SessionEntry::Session {
        version: SESSION_VERSION - 1,
        id,
        timestamp: created_at.to_string(),
        cwd: cwd.to_path_buf(),
        agent_id: None,
        agent_fingerprint: None,
    };
    let message = SessionEntry::Message {
        timestamp: created_at.to_string(),
        message: Message::user_text("bench"),
        display_message: None,
    };
    let contents = format!(
        "{}\n{}\n",
        serde_json::to_string(&header).expect("session header"),
        serde_json::to_string(&message).expect("session message")
    );
    fs::write(&path, contents).expect("write minimal session");
    path
}

fn write_large_transcript(path: &Path, cwd: &Path, entry_count: usize) -> usize {
    let mut contents = String::new();
    let session_id = format!("bench-large-{entry_count}");
    let header = SessionEntry::Session {
        version: SESSION_VERSION,
        id: session_id.clone(),
        timestamp: "1700000000".into(),
        cwd: cwd.to_path_buf(),
        agent_id: None,
        agent_fingerprint: None,
    };
    contents.push_str(&serde_json::to_string(&header).expect("large header"));
    contents.push('\n');

    let session_id = SessionId::from_string(session_id).expect("benchmark session id");
    let history = vec![Message::user_text("fixed history")];
    let mut parent_id = None;
    let mut parent_snapshot = None;
    for index in 0..entry_count {
        let snapshot = SessionSnapshot::new(
            session_id.clone(),
            Revision::from_u64(index as u64 + 1),
            history.clone(),
            ModelIdentity::new("benchmark", "session-summary", "v1"),
            CompactionState::default(),
        );
        let transition = parent_snapshot.as_ref().map_or_else(
            || StoredStateTransition::Snapshot {
                snapshot: Box::new(snapshot.clone()),
            },
            |parent| StoredStateTransition::SnapshotDelta {
                delta: Box::new(
                    StoredSnapshotDelta::after(
                        &SnapshotDeltaBase::from_snapshot(parent),
                        &snapshot,
                    )
                    .expect("constant-history benchmark delta"),
                ),
            },
        );
        let id = NodeId::from_string(format!("node-{index}")).expect("benchmark node id");
        let entry = SessionEntry::Node {
            node: SessionNode {
                id: id.clone(),
                parent_id,
                timestamp: (1_700_000_001 + index as u64).to_string(),
                kind: SessionNodeKind::Commit,
                compaction_facts: None,
                transition,
                display_messages: Vec::new(),
            },
        };
        contents.push_str(&serde_json::to_string(&entry).expect("large entry"));
        contents.push('\n');
        parent_id = Some(id);
        parent_snapshot = Some(snapshot);
    }
    let bytes = contents.len();
    fs::write(path, contents).expect("write large transcript");
    bytes
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

/// Benchmark for `SessionTree::facts()`, which previously scanned every child
/// list on each call. With the cached `branch_count` field it is now O(1).
///
/// Run with:
///   cargo test -p rho-coding-agent --release --lib session::performance_benchmarks::tree_facts_benchmark -- --ignored --nocapture
#[test]
#[ignore = "tree facts benchmark; run with --release --ignored --nocapture"]
fn tree_facts_benchmark() {
    let samples = std::env::var("RHO_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1000)
        .max(10);

    // Build a session with a wide branching tree: one root with many children
    // and each child having one leaf. This makes the old `children.values()
    // .filter(...).count()` scan proportional to node count.
    let root = tempfile::tempdir().expect("session root");
    let cwd = tempfile::tempdir().expect("workspace cwd");
    let session = Session::create_in_root(root.path(), cwd.path()).expect("create session");

    let base = snapshot(
        &session,
        1,
        vec![Message::user_text("root")],
        CompactionState::default(),
    );
    session
        .save_snapshot(&base, base.history())
        .expect("save root snapshot");
    let root_id = session
        .session_tree()
        .expect("tree")
        .active_leaf_id()
        .expect("root leaf")
        .clone();

    const BRANCH_WIDTH: usize = 40;
    for child in 0..BRANCH_WIDTH {
        session.set_leaf(&root_id).expect("set leaf to root");
        let snapshot = snapshot(
            &session,
            2,
            vec![
                Message::user_text("root"),
                Message::assistant_text(format!("branch {child}")),
            ],
            CompactionState::default(),
        );
        session
            .save_snapshot(&snapshot, &snapshot.history()[1..])
            .expect("save branch snapshot");
    }

    // Add a deep chain so the tree has many nodes, making the old scan cost
    // proportional to node count rather than negligible.
    for depth in 0..200 {
        let snapshot = snapshot(
            &session,
            3 + depth as u64,
            vec![
                Message::user_text("root"),
                Message::assistant_text(format!("deep {depth}")),
            ],
            CompactionState::default(),
        );
        session
            .save_snapshot(&snapshot, &snapshot.history()[1..])
            .expect("save deep snapshot");
    }

    let tree = session.session_tree().expect("tree");
    let facts = tree.facts();
    // One root with BRANCH_WIDTH children plus a 200-deep chain from one branch.
    assert_eq!(facts.branch_count, 1);
    eprintln!(
        "tree has {} nodes, {} branches",
        facts.node_count, facts.branch_count,
    );

    // Warmup.
    for _ in 0..WARMUP_ITERS {
        black_box(tree.facts());
    }

    let timing = measure(samples, || tree.facts());
    eprintln!(
        "tree.facts() with {} nodes, {} branches: median {} ns, p95 {} ns",
        facts.node_count,
        facts.branch_count,
        timing.median(),
        timing.percentile(95),
    );

    // Compare against the old scan-everything approach to quantify the win.
    let old_timing = measure(samples, || {
        // Reproduce the old branch_count computation that scanned all child lists.
        let branch_count = tree
            .children_map()
            .values()
            .filter(|children| children.len() > 1)
            .count();
        black_box(branch_count);
    });
    eprintln!(
        "tree.facts() OLD scan: median {} ns, p95 {} ns | NEW cached: median {} ns | \
         speedup {:.1}x",
        old_timing.median(),
        old_timing.percentile(95),
        timing.median(),
        old_timing.median() as f64 / timing.median().max(1) as f64,
    );

    // The cached field makes this O(1). With 41 nodes the old scan was
    // ~hundreds of ns; the cached version should be well under 100 ns.
    assert!(
        timing.median() < 500,
        "tree.facts() regressed: median {} ns exceeds 500 ns budget",
        timing.median(),
    );
}

fn snapshot(
    session: &Session,
    revision: u64,
    history: Vec<Message>,
    compaction: CompactionState,
) -> SessionSnapshot {
    SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(revision),
        history,
        ModelIdentity::new("provider", "api", "model"),
        compaction,
    )
    .with_prompt_cache_key(format!("rho:{}", session.id()))
}
