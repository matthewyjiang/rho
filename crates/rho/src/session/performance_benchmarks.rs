//! Ignored optimized-test instrumentation for private session hot paths.
//!
//! Fixtures are built outside timed samples. Measured call sites keep exact
//! production behavior; this module does not change public APIs.

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

/// Practical fixed workspace size for warm list / index sync measurements.
const LIST_SESSION_COUNT: usize = 750;
/// Thousands of small transcript entries to expose summarize double-parse cost.
const SUMMARIZE_ENTRY_COUNT: usize = 4_000;
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
    // Seed the workspace index outside timed samples so measurements stay warm.
    let seeded = Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
        .expect("seed list_in_root");
    assert_eq!(
        seeded.len(),
        list_fixture.session_count,
        "fixture session count must match list results before timing"
    );
    for _ in 0..WARMUP_ITERS {
        black_box(
            Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
                .expect("warmup list_in_root"),
        );
    }
    let list_timing = measure(samples, || {
        Session::list_in_root(list_fixture.root.path(), list_fixture.cwd.path())
            .expect("timed list_in_root")
    });

    let summarize_fixture = SummarizeFixture::build(SUMMARIZE_ENTRY_COUNT);
    for _ in 0..WARMUP_ITERS {
        black_box(
            summarize_session_file(&summarize_fixture.path, summarize_fixture.cwd.path())
                .expect("warmup summarize_session_file"),
        );
    }
    let summarize_timing = measure(samples, || {
        summarize_session_file(&summarize_fixture.path, summarize_fixture.cwd.path())
            .expect("timed summarize_session_file")
    });

    let report = json!({
        "schema_version": 1,
        "suite": "rho-session-hot-path-benchmarks",
        "profile": "test with opt-level=3",
        "sample_count": samples,
        "candidate_commit": command_output("git", &["rev-parse", "--short", "HEAD"]),
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "measurements": {
            "list_in_root_warm": {
                "session_count": list_fixture.session_count,
                "timing": list_timing.json(),
            },
            "summarize_session_file": {
                "entry_count": summarize_fixture.entry_count,
                "bytes": summarize_fixture.bytes,
                "timing": summarize_timing.json(),
            },
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
}

impl ListFixture {
    fn build(session_count: usize) -> Self {
        let root = tempfile::tempdir().expect("session root");
        let cwd = tempfile::tempdir().expect("workspace cwd");
        for index in 0..session_count {
            write_minimal_session(root.path(), cwd.path(), index);
        }
        Self {
            root,
            cwd,
            session_count,
        }
    }
}

struct SummarizeFixture {
    _root: TempDir,
    cwd: TempDir,
    path: PathBuf,
    entry_count: usize,
    bytes: usize,
}

impl SummarizeFixture {
    fn build(entry_count: usize) -> Self {
        let root = tempfile::tempdir().expect("summarize session root");
        let cwd = tempfile::tempdir().expect("summarize cwd");
        let workspace = session_dir_in_root(root.path(), cwd.path());
        fs::create_dir_all(&workspace).expect("summarize workspace dir");
        let session_dir = workspace.join("1700000000_bench-large-transcript");
        fs::create_dir_all(&session_dir).expect("summarize session dir");
        let path = session_dir.join(SESSION_TRANSCRIPT_FILE_NAME);
        let bytes = write_large_transcript(&path, cwd.path(), entry_count);
        Self {
            _root: root,
            cwd,
            path,
            entry_count,
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

    fn json(&self) -> Value {
        json!({
            "unit": "nanoseconds",
            "samples": self.samples_ns,
            "median": self.percentile(50),
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

fn write_minimal_session(session_root: &Path, cwd: &Path, index: usize) {
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
    fs::write(path, contents).expect("write minimal session");
}

fn write_large_transcript(path: &Path, cwd: &Path, entry_count: usize) -> usize {
    let mut contents = String::new();
    let session_id = "bench-large-transcript";
    let header = SessionEntry::Session {
        version: SESSION_VERSION,
        id: session_id.into(),
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
