//! Opt-in product benchmark. Copies only transcripts into a private temporary
//! root; never writes a real session/index or prints private source text.

use std::{fs, path::PathBuf, time::Instant};

use serde_json::json;
use tempfile::TempDir;

use super::search::{execute, Request};
use super::{layout::SessionUnit, Session};

#[test]
#[ignore = "manual aggregate-only benchmark over an explicitly supplied corpus"]
fn sessions_search_product_benchmark() {
    let source = PathBuf::from(
        std::env::var_os("RHO_SEARCH_BENCH_ROOT").expect("set RHO_SEARCH_BENCH_ROOT explicitly"),
    );
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let mut raw_bytes = 0;
    let mut files = 0;
    for workspace in fs::read_dir(&source).unwrap() {
        let workspace = workspace.unwrap();
        if !workspace.file_type().unwrap().is_dir() {
            continue;
        }
        for entry in fs::read_dir(workspace.path()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_symlink() {
                continue;
            }
            let Some(unit) = SessionUnit::from_path(&entry.path()) else {
                continue;
            };
            let path = unit.transcript_path();
            if fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
            {
                continue;
            }
            let destination = root.path().join(path.strip_prefix(&source).unwrap());
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            raw_bytes += fs::copy(path, destination).unwrap();
            files += 1;
        }
    }
    let cancellation = rho_sdk::CancellationToken::new();
    let call = |args| {
        let request: Request = serde_json::from_value(args).unwrap();
        execute(
            root.path(),
            cwd.path(),
            "benchmark-current",
            request,
            rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
            &cancellation,
        )
        .unwrap()
    };
    let start = Instant::now();
    let bootstrap = call(json!({"action":"search","query":"zzabsentneedlezz","scope":"all"}));
    let build_ms = start.elapsed().as_secs_f64() * 1000.0;
    let bootstrap: serde_json::Value = serde_json::from_str(&bootstrap).unwrap();
    assert_eq!(bootstrap["index"]["skipped_files"], 0);
    let database_bytes = fs::metadata(root.path().join("search.sqlite3"))
        .unwrap()
        .len();
    let queries = [
        "compaction",
        "session index",
        "E0308",
        "cargo test",
        "workspace",
        "cancellation",
        "zzabsentneedlezz",
    ];
    let mut cold = Vec::new();
    let mut warm = Vec::new();
    let mut bytes = Vec::new();
    let mut example = None;
    for query in queries {
        for iteration in 0..11 {
            let start = Instant::now();
            let output = call(json!({"action":"search","query":query,"scope":"all"}));
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if iteration == 0 {
                cold.push(elapsed);
            } else {
                warm.push(elapsed);
            }
            bytes.push(output.len());
            let output: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(output["index"]["files_checked"], 0);
            assert_eq!(output["index"]["bytes_read"], 0);
            if example.is_none() {
                example = output["sessions"].as_array().unwrap().first().cloned();
            }
        }
    }
    let group = example.expect("at least one benchmark query should match");
    let start = Instant::now();
    let read = call(
        json!({"action":"read","scope":"all","session":group["session"],"anchor":group["excerpts"][0]["anchor"],"start":group["excerpts"][0]["start"]}),
    );
    let read_ms = start.elapsed().as_secs_f64() * 1000.0;
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&rho_providers::model::Message::user_text(
            "incrementalbenchevidence",
        ))
        .unwrap();
    let start = Instant::now();
    let incremental =
        call(json!({"action":"search","query":"incrementalbenchevidence","scope":"all"}));
    let incremental_ms = start.elapsed().as_secs_f64() * 1000.0;
    let incremental: serde_json::Value = serde_json::from_str(&incremental).unwrap();
    assert_eq!(incremental["index"]["files_checked"], 1);
    assert_eq!(incremental["sessions"][0]["id"], session.id());
    cold.sort_by(f64::total_cmp);
    warm.sort_by(f64::total_cmp);
    bytes.sort_unstable();
    println!(
        "SESSIONS_SEARCH_BENCH {}",
        json!({
            "files":files,"raw_bytes":raw_bytes,"build_ms":build_ms,"database_bytes":database_bytes,
            "bootstrap":bootstrap["index"],"cold_connection_p50_ms":cold[cold.len()/2],
            "cold_connection_p95_ms":cold[cold.len()*95/100],"warm_os_cache_p50_ms":warm[warm.len()/2],
            "warm_os_cache_p95_ms":warm[warm.len()*95/100],"response_bytes_p50":bytes[bytes.len()/2],
            "response_bytes_p95":bytes[bytes.len()*95/100],"read_ms":read_ms,"read_response_bytes":read.len(),
            "incremental_ms":incremental_ms,"incremental":incremental["index"],
            "note":"debug build; fresh SQLite connection for each call, OS cache not flushed; no private text emitted"
        })
    );
}
