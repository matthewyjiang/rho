//! Fail-closed blocking dispatch and non-blocking observation.
#![cfg(unix)]

use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::hooks::{HookPayloadBounds, HookPolicyOutcome};
use tempfile::TempDir;

use super::*;
use crate::hooks::catalog::ProjectTrust;

/// Builds a hooks file whose handlers are inline shell scripts.
struct Fixture {
    home: TempDir,
    entries: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
            entries: Vec::new(),
        }
    }

    fn hook(mut self, id: &str, event: &str, timeout: &str, script: &str) -> Self {
        let program = self.home.path().join(format!("{id}.sh"));
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        // Run the script through an interpreter rather than executing the file
        // directly: a write fd held by a concurrent test thread is inherited
        // across fork and makes execve of a fresh script fail with ETXTBSY.
        self.entries.push(format!(
            "[[hook]]\nid = \"{id}\"\non = \"{event}\"\ncommand = [\"/bin/sh\", \"{}\"]\ntimeout = \"{timeout}\"\n",
            program.display()
        ));
        self
    }

    fn engine(self) -> Arc<HookEngine> {
        std::fs::write(
            self.home.path().join("hooks.toml"),
            format!("version = 1\n\n{}", self.entries.join("\n")),
        )
        .unwrap();
        let catalog =
            HookCatalog::discover(Some(self.home.path()), None, ProjectTrust::Untrusted).unwrap();
        // The TempDir must outlive the engine, so leak it for the test's lifetime.
        std::mem::forget(self.home);
        Arc::new(HookEngine::new(catalog, HookPayloadBounds::default()))
    }
}

const CONTINUE: &str = r#"cat > /dev/null; echo '{"version":1,"decision":"continue"}'"#;
const DENY: &str =
    r#"cat > /dev/null; echo '{"version":1,"decision":"deny","reason":"force push"}'"#;

/// Builds a `before_tool_use` request for tool `bash` through the SDK's own
/// constructor so the payload shape stays honest.
fn request() -> PreToolUseRequest {
    rho_sdk::hooks::testing::before_tool_use_request("bash", HookPolicyOutcome::Allow)
}

async fn decide(engine: Arc<HookEngine>) -> HookDecision {
    CommandHookGate::new(engine).evaluate(request()).await
}

#[tokio::test]
async fn no_matching_hook_lets_the_call_continue() {
    let engine = Fixture::new()
        .hook("post", "after_tool_use", "5s", "true")
        .engine();

    assert_eq!(decide(engine).await, HookDecision::Continue);
}

#[tokio::test]
async fn a_valid_continue_lets_the_call_proceed() {
    let engine = Fixture::new()
        .hook("gate", "before_tool_use", "10s", CONTINUE)
        .engine();

    assert_eq!(decide(Arc::clone(&engine)).await, HookDecision::Continue);
    assert_eq!(engine.activity()[0].outcome.label(), "continued");
}

#[tokio::test]
async fn a_valid_deny_stops_the_call_and_names_the_hook() {
    let engine = Fixture::new()
        .hook("gate", "before_tool_use", "10s", DENY)
        .engine();

    let decision = decide(Arc::clone(&engine)).await;

    assert_eq!(
        decision,
        HookDecision::deny("denied by hook `user:gate`: force push")
    );
    assert_eq!(engine.activity()[0].hook_id, "user:gate");
}

#[tokio::test]
async fn hooks_run_in_configured_order_and_the_first_denial_wins() {
    let engine = Fixture::new()
        .hook("first", "before_tool_use", "10s", CONTINUE)
        .hook("second", "before_tool_use", "10s", DENY)
        .hook("third", "before_tool_use", "10s", DENY)
        .engine();

    let decision = decide(Arc::clone(&engine)).await;

    assert_eq!(
        decision.denial_reason(),
        Some("denied by hook `user:second`: force push")
    );
    assert_eq!(
        engine
            .activity()
            .into_iter()
            .map(|record| record.hook_id)
            .collect::<Vec<_>>(),
        vec!["user:first", "user:second"],
        "dispatch stops at the first denial"
    );
}

#[tokio::test]
async fn a_timeout_denies_and_says_so() {
    let engine = Fixture::new()
        .hook("slow", "before_tool_use", "1s", "sleep 30")
        .engine();

    let decision = decide(engine).await;

    let reason = decision.denial_reason().expect("a timeout must deny");
    assert!(reason.contains("user:slow"), "{reason}");
    assert!(reason.contains("timed out"), "{reason}");
}

#[tokio::test]
async fn a_crash_denies_and_reports_the_exit_status() {
    let engine = Fixture::new()
        .hook(
            "broken",
            "before_tool_use",
            "10s",
            "echo 'handler exploded' >&2; exit 9",
        )
        .engine();

    let reason = decide(engine)
        .await
        .denial_reason()
        .expect("a crash must deny")
        .to_owned();

    assert!(reason.contains("user:broken"), "{reason}");
    assert!(reason.contains("status 9"), "{reason}");
    assert!(reason.contains("handler exploded"), "{reason}");
}

#[tokio::test]
async fn malformed_output_denies_rather_than_continuing() {
    let engine = Fixture::new()
        .hook("garbled", "before_tool_use", "10s", "echo not-json")
        .engine();

    let reason = decide(engine)
        .await
        .denial_reason()
        .expect("malformed output must deny")
        .to_owned();

    assert!(reason.contains("user:garbled"), "{reason}");
    assert!(reason.contains("not valid JSON"), "{reason}");
}

#[tokio::test]
async fn silence_denies_rather_than_continuing() {
    let engine = Fixture::new()
        .hook("silent", "before_tool_use", "10s", "true")
        .engine();

    let reason = decide(engine)
        .await
        .denial_reason()
        .expect("silence must deny")
        .to_owned();

    assert!(reason.contains("wrote no decision"), "{reason}");
}

#[tokio::test]
async fn a_wrong_schema_version_denies() {
    let engine = Fixture::new()
        .hook(
            "future",
            "before_tool_use",
            "10s",
            r#"echo '{"version":99,"decision":"continue"}'"#,
        )
        .engine();

    let reason = decide(engine)
        .await
        .denial_reason()
        .expect("a schema mismatch must deny")
        .to_owned();

    assert!(reason.contains("schema version 99"), "{reason}");
}

#[tokio::test]
async fn a_nonzero_exit_that_still_writes_a_valid_deny_is_honored() {
    let engine = Fixture::new()
        .hook(
            "strict",
            "before_tool_use",
            "10s",
            r#"echo '{"version":1,"decision":"deny","reason":"blocked"}'; exit 1"#,
        )
        .engine();

    assert_eq!(
        decide(engine).await.denial_reason(),
        Some("denied by hook `user:strict`: blocked")
    );
}

#[tokio::test]
async fn a_reload_cannot_change_the_hook_set_midway_through_a_dispatch() {
    let engine = Fixture::new()
        .hook(
            "slow",
            "before_tool_use",
            "10s",
            r#"sleep 1; echo '{"version":1,"decision":"deny","reason":"late"}'"#,
        )
        .engine();
    let replacement = HookCatalog::default();

    let gate = CommandHookGate::new(Arc::clone(&engine));
    let dispatch = gate.evaluate(request());
    tokio::pin!(dispatch);
    // Start the dispatch, then swap the catalog while it is in flight.
    let decision = tokio::join!(async { (&mut dispatch).await }, async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        engine.reload(replacement);
    })
    .0;

    assert!(
        decision.is_deny(),
        "the snapshot taken at dispatch start must still be used"
    );
    assert!(engine.catalog().is_empty());
}

#[tokio::test]
async fn an_observational_failure_is_recorded_but_never_denies() {
    let engine = Fixture::new()
        .hook("post", "after_tool_use", "10s", "exit 4")
        .engine();
    let (observer, worker) =
        observational_channel(Arc::clone(&engine), rho_sdk::CancellationToken::new());

    observer.observe(rho_sdk::hooks::testing::after_tool_use_envelope("bash"));
    drop(observer);
    worker.drain(Duration::from_secs(10)).await;

    let activity = engine.activity();
    assert_eq!(activity[0].hook_id, "user:post");
    assert_eq!(activity[0].outcome.label(), "failed");
}

#[tokio::test]
async fn an_observational_success_is_recorded() {
    let engine = Fixture::new()
        .hook("post", "after_tool_use", "10s", "cat > /dev/null")
        .engine();
    let (observer, worker) =
        observational_channel(Arc::clone(&engine), rho_sdk::CancellationToken::new());

    observer.observe(rho_sdk::hooks::testing::after_tool_use_envelope("bash"));
    drop(observer);
    worker.drain(Duration::from_secs(10)).await;

    assert_eq!(engine.activity()[0].outcome.label(), "observed");
}

// Covers: one waiting observer must not prevent an independent observer from running.
// Owner: host observational dispatcher.
#[tokio::test]
async fn observational_handlers_are_isolated_from_each_other() {
    let fixture = Fixture::new();
    let signal = fixture.home.path().join("observer-signal");
    assert!(std::process::Command::new("mkfifo")
        .arg(&signal)
        .status()
        .unwrap()
        .success());
    let signal = signal.display();
    let engine = fixture
        .hook(
            "waiting",
            "after_tool_use",
            "2s",
            &format!("read ready < '{signal}'"),
        )
        .hook(
            "signalling",
            "after_tool_use",
            "2s",
            &format!("printf 'ready\\n' > '{signal}'"),
        )
        .engine();
    let (observer, worker) =
        observational_channel(Arc::clone(&engine), rho_sdk::CancellationToken::new());

    observer.observe(rho_sdk::hooks::testing::after_tool_use_envelope("bash"));
    drop(observer);
    worker.drain(Duration::from_secs(3)).await;

    let mut activity = engine.activity();
    activity.sort_by(|left, right| left.hook_id.cmp(&right.hook_id));
    assert_eq!(
        activity
            .into_iter()
            .map(|record| (record.hook_id, record.outcome.label()))
            .collect::<Vec<_>>(),
        vec![
            ("user:signalling".into(), "observed"),
            ("user:waiting".into(), "observed"),
        ]
    );
}

#[tokio::test]
async fn an_event_no_hook_matches_records_nothing() {
    let engine = Fixture::new()
        .hook("post", "after_tool_use", "10s", "true")
        .engine();
    let (observer, worker) =
        observational_channel(Arc::clone(&engine), rho_sdk::CancellationToken::new());

    observer.observe(rho_sdk::hooks::testing::run_completed_envelope());
    drop(observer);
    worker.drain(Duration::from_secs(10)).await;

    assert!(engine.activity().is_empty());
}

// Covers: shutdown grace expiry must cancel the owned worker instead of detaching it.
// Owner: host observational dispatcher.
#[tokio::test]
async fn drain_cancels_the_worker_after_its_grace_expires() {
    let engine = Fixture::new()
        .hook("post", "after_tool_use", "10s", "sleep 10")
        .engine();
    let cancellation = rho_sdk::CancellationToken::new();
    let (observer, worker) = observational_channel(Arc::clone(&engine), cancellation.clone());

    observer.observe(rho_sdk::hooks::testing::after_tool_use_envelope("bash"));
    drop(observer);
    worker.drain(Duration::ZERO).await;

    assert!(cancellation.is_cancelled());
}

// Covers: pipeline shutdown must close its queue without waiting for SDK observer clones.
// Owner: host observational dispatcher.
#[tokio::test]
async fn drain_owns_queue_closure_even_while_an_observer_is_attached() {
    let engine = Arc::new(HookEngine::new(
        HookCatalog::default(),
        HookPayloadBounds::default(),
    ));
    let cancellation = rho_sdk::CancellationToken::new();
    let (observer, worker) = observational_channel(Arc::clone(&engine), cancellation.clone());

    worker.drain(Duration::from_secs(1)).await;

    assert!(!cancellation.is_cancelled());
    observer.observe(rho_sdk::hooks::testing::run_completed_envelope());
    assert_eq!(engine.activity()[0].outcome.label(), "dropped");
}
