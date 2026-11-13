//! What a loaded session costs the daemon in memory (#1927): its history,
//! its saved tree and the transcript its clients replay share one copy of
//! the conversation, rather than each holding its own.

use serde_json::json;

use super::sessions::{settled_job, Sandbox};

/// How many turns the session takes, each a shell call whose output the
/// fixture answers with, so each turn saves its output twice.
const TURNS: usize = 40;

/// Bytes of output each turn's shell call prints: base64 of this many
/// random bytes, in lines of 200 characters.
const RANDOM_BYTES: usize = 375_000;

/// How far the daemon's memory may grow per byte the session saved. One
/// copy of the conversation is one byte per byte saved; the replay buffer,
/// whose size is fixed (`REPLAY_BUFFER_BYTES`), and the session's other
/// state stay well under another. Measured over three runs: 1.53 to 1.58
/// with #1927's fix; 3.11 with only the SDK's history shared; 7.11 before.
const MAX_GROWTH_PER_SAVED_BYTE: f64 = 2.0;

/// The daemon's resident anonymous memory, in bytes.
fn anonymous_memory(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("RssAnon:"))
        .and_then(|value| value.trim().strip_suffix("kB"))
        .and_then(|value| value.trim().parse().ok())
        .expect("RssAnon in /proc/<pid>/status");
    kib * 1024
}

// Covers: a loaded session takes about one copy of its conversation in the
// daemon. Before #1927 the SDK's history, every save's snapshot, the
// session file's cached tree and the feed's replay transcript each held a
// copy of every tool output, about 7 bytes per byte saved, so a long thread
// took hundreds of megabytes until it unloaded.
//
// Measured as resident memory, with glibc told to serve every allocation
// above 64 KiB from its own mapping, so freed copies leave at once: what
// stays resident is what the session holds. glibc only, like the setting.
// Owner: daemon session memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loaded_session_holds_about_one_copy_of_its_conversation() {
    let sandbox = Sandbox::new();
    sandbox.set_config("bypass", "[output]\nmax_output_bytes = 1048576\n");
    let mut command = sandbox.rho("tool-call");
    command
        .env("MALLOC_MMAP_THRESHOLD_", "65536")
        .env("MALLOC_ARENA_MAX", "2");
    let (daemon, client) = sandbox.spawn_daemon(command, &[]).await;
    let session = sandbox.new_session(&client).await;
    let before = anonymous_memory(daemon.id());

    for turn in 0..TURNS {
        let call = json!({
            "id": format!("call-{turn}"),
            "name": "bash",
            "arguments": {
                "command": format!("head -c {RANDOM_BYTES} /dev/urandom | base64 -w 200"),
            },
        });
        sandbox.prompt_tool_call(&client, &session, &call).await;
        settled_job(&sandbox, &client, &session).await;
    }
    let grown = anonymous_memory(daemon.id()).saturating_sub(before);
    let saved = std::fs::metadata(
        sandbox
            .session_dir(&session)
            .expect("the session saved")
            .join("session.jsonl"),
    )
    .unwrap()
    .len();

    // Each turn saves its output twice, as the result and as the reply, so
    // a shorter file means turns that did not run as the ratio assumes.
    let printed = TURNS * RANDOM_BYTES * 4 / 3;
    assert!(
        saved >= 2 * printed as u64,
        "the session saved {saved} bytes for {printed} bytes of output; did every turn run?"
    );
    let per_saved_byte = grown as f64 / saved as f64;
    assert!(
        per_saved_byte <= MAX_GROWTH_PER_SAVED_BYTE,
        "the daemon grew {grown} bytes for a session that saved {saved} bytes: \
         {per_saved_byte:.2} per byte saved, over {MAX_GROWTH_PER_SAVED_BYTE}"
    );
}
