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

    /// Returns the engine with its home dir; hold the `TempDir` for the test's lifetime.
    fn engine(self) -> (TempDir, Arc<HookEngine>) {
        std::fs::write(
            self.home.path().join("hooks.toml"),
            format!("version = 1\n\n{}", self.entries.join("\n")),
        )
        .unwrap();
        let catalog =
            HookCatalog::discover(Some(self.home.path()), None, ProjectTrust::Untrusted).unwrap();
        (
            self.home,
            Arc::new(HookEngine::new(catalog, HookPayloadBounds::default())),
        )
    }
}

const CONTINUE: &str = r#"cat > /dev/null; echo '{"version":1,"decision":"continue"}'"#;
const DENY: &str =
    r#"cat > /dev/null; echo '{"version":1,"decision":"deny","reason":"force push"}'"#;

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum NodeOutcome {
    Success,
}

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
    let (_home, engine) = Fixture::new()
        .hook("post", "after_tool_use", "5s", "true")
        .engine();

    assert_eq!(decide(engine).await, HookDecision::Continue);
}

#[tokio::test]
async fn a_valid_continue_lets_the_call_proceed() {
    let (_home, engine) = Fixture::new()
        .hook("gate", "before_tool_use", "10s", CONTINUE)
        .engine();

    assert_eq!(decide(Arc::clone(&engine)).await, HookDecision::Continue);
    assert_eq!(engine.activity()[0].outcome.label(), "continued");
}

#[tokio::test]
async fn hooks_run_in_configured_order_and_the_first_denial_wins() {
    let (_home, engine) = Fixture::new()
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

// Covers: every way a blocking handler can fail denies the call (fail
// closed) and the reason names the hook and the failure.
// Owner: host blocking dispatcher.
#[tokio::test]
async fn a_failing_handler_denies_rather_than_continuing() {
    for (id, timeout, script, expected) in [
        ("slow", "1s", "sleep 30", &["user:slow", "timed out"][..]),
        (
            "broken",
            "10s",
            "echo 'handler exploded' >&2; exit 9",
            &["user:broken", "status 9", "handler exploded"][..],
        ),
        (
            "garbled",
            "10s",
            "echo not-json",
            &["user:garbled", "not valid JSON"][..],
        ),
        (
            "silent",
            "10s",
            "true",
            &["user:silent", "wrote no decision"][..],
        ),
        (
            "future",
            "10s",
            r#"echo '{"version":99,"decision":"continue"}'"#,
            &["user:future", "schema version 99"][..],
        ),
    ] {
        let (_home, engine) = Fixture::new()
            .hook(id, "before_tool_use", timeout, script)
            .engine();

        let decision = decide(engine).await;

        let reason = decision
            .denial_reason()
            .unwrap_or_else(|| panic!("{id} must deny"));
        for fragment in expected {
            assert!(reason.contains(fragment), "{id}: {reason}");
        }
    }
}

#[tokio::test]
async fn a_nonzero_exit_that_still_writes_a_valid_deny_is_honored() {
    let (_home, engine) = Fixture::new()
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
    use std::{future::Future, task::Poll};

    let (_home, engine) = Fixture::new()
        .hook("gate", "before_tool_use", "10s", DENY)
        .engine();
    let replacement = HookCatalog::default();

    let gate = CommandHookGate::new(Arc::clone(&engine));
    let mut dispatch = std::pin::pin!(gate.evaluate(request()));
    // The first poll takes the catalog snapshot and starts the hook process;
    // swap the catalog after that, while the dispatch is still in flight.
    let first = std::future::poll_fn(|cx| Poll::Ready(dispatch.as_mut().poll(cx))).await;
    engine.reload(replacement);
    let decision = match first {
        Poll::Ready(decision) => decision,
        Poll::Pending => dispatch.await,
    };

    assert!(
        decision.is_deny(),
        "the snapshot taken at dispatch start must still be used"
    );
    assert!(engine.catalog().is_empty());
}

#[tokio::test]
async fn an_observational_failure_is_recorded_but_never_denies() {
    let (_home, engine) = Fixture::new()
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

// Covers: a workflow hook crash and stdout must stay observational.
// Owner: app observational dispatcher.
#[tokio::test]
async fn workflow_hook_failure_and_output_do_not_return_to_the_caller() {
    let fixture = Fixture::new();
    let recorded = fixture.home.path().join("workflow-event.json");
    let (_home, engine) = fixture
        .hook(
            "workflow",
            "workflow_node_finished",
            "10s",
            &format!(
                "cat > '{}'; printf 'not workflow data'; exit 7",
                recorded.display()
            ),
        )
        .engine();
    let (_observer, worker) =
        observational_channel(Arc::clone(&engine), rho_sdk::CancellationToken::new());

    engine.observe_workflow_node_finished(super::WorkflowNodeFinished {
        workflow_run_id: "run-1",
        plan_digest: "digest-1",
        node_id: "node-1",
        attempt: 2,
        outcome: &NodeOutcome::Success,
        duration: Duration::from_millis(41),
        artifacts: &[crate::workflow::DurableArtifactReference {
            kind: crate::workflow::ArtifactKind::Stdout,
            artifact: crate::workflow::ArtifactRef {
                relative_path: "artifacts/stdout".into(),
                retained_bytes: 4,
                observed: crate::workflow::ArtifactObservation::Complete { observed_bytes: 4 },
                digest: crate::workflow::Digest("sha256:test".into()),
            },
        }],
    });
    worker.drain(Duration::from_secs(10)).await;

    let event: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(recorded).unwrap()).unwrap();
    assert_eq!(event["event"], "workflow_node_finished");
    assert_eq!(
        event["payload"],
        serde_json::json!({
            "workflow_run_id": "run-1",
            "plan_digest": "digest-1",
            "node_id": "node-1",
            "attempt": 2,
            "outcome": "success",
            "duration_ms": 41,
            "artifact_references": [{
                "kind": "stdout",
                "relative_path": "artifacts/stdout",
                "retained_bytes": 4,
                "observed": {"kind": "complete", "observed_bytes": 4},
                "digest": "sha256:test"
            }]
        })
    );
    assert_eq!(engine.activity()[0].outcome.label(), "failed");
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
    let (_home, engine) = fixture
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

// Covers: shutdown grace expiry must cancel the owned worker instead of detaching it.
// Owner: host observational dispatcher.
#[tokio::test]
async fn drain_cancels_the_worker_after_its_grace_expires() {
    let (_home, engine) = Fixture::new()
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
