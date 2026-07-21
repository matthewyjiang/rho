use std::{
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use pretty_assertions::assert_eq;
use proptest::prelude::*;
use tempfile::TempDir;

use super::{
    approval_channel, authorize, authorize_for_call, ApprovalAuditDecision, ApprovalAuditLog,
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CapabilityKind,
    CapabilityOperation, CapabilityRequest, CapabilitySource, DenyAllPolicy, DenyApprovals,
    NetworkTarget, PathScope, PolicyDecision, ProcessEnvironment, ProcessExecution,
    ProcessInvocation, ProcessOutputLimits, ScopedWorkspacePolicy, SessionApprovals, Workspace,
    WorkspacePathErrorKind, WorkspacePathState, WorkspacePolicy,
};

fn source(name: &str) -> CapabilitySource {
    CapabilitySource::built_in_tool(name)
}

fn read_request(path: impl Into<PathBuf>) -> CapabilityRequest {
    CapabilityRequest::read_path(path, PathScope::PrimaryWorkspace, source("read_file"))
}

fn process_request(command: &str) -> CapabilityRequest {
    CapabilityRequest::process(
        ProcessExecution::new(
            "/workspace",
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
            ProcessEnvironment::InheritAll,
            ProcessOutputLimits::new(4096, Some(Duration::from_secs(30))),
        ),
        source("bash"),
    )
}

#[test]
fn workspace_defines_absolute_parent_missing_and_platform_path_behavior() {
    assert_eq!(
        Workspace::new("relative").unwrap_err().kind(),
        WorkspacePathErrorKind::RootNotAbsolute
    );
    let root = TempDir::new().unwrap();
    let workspace = Workspace::new(root.path()).unwrap();

    assert_eq!(
        workspace.resolve("src/lib.rs").unwrap(),
        workspace.root().join("src/lib.rs")
    );
    assert_eq!(
        workspace.resolve("../secret").unwrap_err().kind(),
        WorkspacePathErrorKind::ParentTraversal
    );
    assert_eq!(
        workspace
            .resolve(root.path().with_extension("outside"))
            .unwrap_err()
            .kind(),
        WorkspacePathErrorKind::OutsideGrantedRoots
    );
    assert_eq!(
        workspace.resolve_for_read("missing").unwrap_err().kind(),
        WorkspacePathErrorKind::Missing
    );
    let missing = workspace.resolve_for_write("new/child.txt").unwrap();
    assert_eq!(missing.state(), WorkspacePathState::MissingWriteTarget);
    assert_eq!(missing.scope(), &PathScope::PrimaryWorkspace);
    assert!(Workspace::new(root.path().join("missing")).is_err());

    #[cfg(unix)]
    assert_eq!(
        workspace
            .resolve(PathBuf::from("bad\0path"))
            .unwrap_err()
            .kind(),
        WorkspacePathErrorKind::InvalidPlatformPath
    );
}

proptest! {
    #[test]
    fn any_parent_component_is_rejected(
        prefix in prop::collection::vec("[a-zA-Z0-9_-]{1,12}", 0..5),
        suffix in prop::collection::vec("[a-zA-Z0-9_-]{1,12}", 0..5),
    ) {
        let root = TempDir::new().unwrap();
        let workspace = Workspace::new(root.path()).unwrap();
        let mut path = PathBuf::new();
        path.extend(prefix);
        path.push("..");
        path.extend(suffix);
        prop_assert_eq!(
            workspace.resolve(path).unwrap_err().kind(),
            WorkspacePathErrorKind::ParentTraversal
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinks_require_deliberate_outside_root_grants_and_policy_grants() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();

    let workspace = Workspace::new(root.path()).unwrap();
    assert_eq!(
        workspace
            .resolve_for_read("escape/secret")
            .unwrap_err()
            .kind(),
        WorkspacePathErrorKind::OutsideGrantedRoots
    );

    let workspace = workspace.with_granted_root(outside.path()).unwrap();
    let resolved = workspace.resolve_for_read("escape/secret").unwrap();
    assert_eq!(
        resolved.scope(),
        &PathScope::GrantedRoot {
            root: std::fs::canonicalize(outside.path()).unwrap(),
        }
    );
    let request = CapabilityRequest::read_path(
        resolved.path(),
        resolved.scope().clone(),
        source("read_file"),
    );
    assert!(matches!(
        ScopedWorkspacePolicy::new()
            .allow_read_paths()
            .evaluate(&request),
        PolicyDecision::Deny { .. }
    ));
    assert_eq!(
        ScopedWorkspacePolicy::new()
            .allow_read_paths()
            .allow_outside_workspace_paths()
            .evaluate(&request),
        PolicyDecision::Allow
    );
}

#[cfg(unix)]
#[test]
fn absolute_paths_match_granted_roots_after_symlink_normalization() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let file = outside.path().join("granted.txt");
    std::fs::write(&file, "granted").unwrap();
    let alias_parent = TempDir::new().unwrap();
    let alias = alias_parent.path().join("alias");
    symlink(outside.path(), &alias).unwrap();

    let workspace = Workspace::new(root.path())
        .unwrap()
        .with_granted_root(outside.path())
        .unwrap();
    let resolved = workspace
        .resolve_for_read(alias.join("granted.txt"))
        .unwrap();
    assert_eq!(
        resolved.scope(),
        &PathScope::GrantedRoot {
            root: std::fs::canonicalize(outside.path()).unwrap(),
        }
    );
}

#[cfg(unix)]
#[test]
fn write_resolution_rejects_broken_symlinks_instead_of_authorizing_them() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let missing_outside = outside.path().join("secret.txt");
    symlink(&missing_outside, root.path().join("out")).unwrap();

    let workspace = Workspace::new(root.path()).unwrap();
    assert_eq!(
        workspace.resolve_for_write("out").unwrap_err().kind(),
        WorkspacePathErrorKind::Missing
    );
}

#[cfg(unix)]
#[test]
fn write_resolution_canonicalizes_parent_and_detects_post_approval_change() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::create_dir(root.path().join("safe")).unwrap();
    let workspace = Workspace::new(root.path()).unwrap();
    let resolved = workspace.resolve_for_write("safe/new.txt").unwrap();
    std::fs::remove_dir(root.path().join("safe")).unwrap();
    symlink(outside.path(), root.path().join("safe")).unwrap();

    assert!(matches!(
        workspace.revalidate(&resolved).unwrap_err().kind(),
        WorkspacePathErrorKind::OutsideGrantedRoots
            | WorkspacePathErrorKind::ChangedAfterAuthorization
    ));
}

#[test]
fn capabilities_are_independent_and_network_urls_are_strict() {
    let policy = ScopedWorkspacePolicy::new()
        .allow_read_paths()
        .allow_skills()
        .allow_network_host("example.com");
    assert_eq!(
        policy.evaluate(&read_request("/workspace/file")),
        PolicyDecision::Allow
    );
    assert_eq!(
        policy.evaluate(&CapabilityRequest::skill("test", None, source("skill"))),
        PolicyDecision::Allow
    );
    for request in [
        CapabilityRequest::write_path(
            "/workspace/file",
            PathScope::PrimaryWorkspace,
            source("write_file"),
        ),
        process_request("cargo test"),
        CapabilityRequest::instruction_discovery(
            "/workspace/AGENTS.md",
            PathScope::PrimaryWorkspace,
            CapabilitySource::PromptConstruction,
        ),
    ] {
        assert!(matches!(
            policy.evaluate(&request),
            PolicyDecision::Deny { .. }
        ));
    }

    for url in ["https://example.com/path", "https://EXAMPLE.com./path"] {
        assert_eq!(
            policy.evaluate(&CapabilityRequest::network(
                NetworkTarget::Url(url.into()),
                source("fetch_content"),
            )),
            PolicyDecision::Allow
        );
    }
    for url in [
        "https://user:secret@example.com/path",
        "file:///etc/passwd",
        "https://example.com.evil.test/path",
        "not a url",
    ] {
        assert!(matches!(
            policy.evaluate(&CapabilityRequest::network(
                NetworkTarget::Url(url.into()),
                source("fetch_content"),
            )),
            PolicyDecision::Deny { .. }
        ));
    }
}

#[test]
fn default_policy_denies_every_capability_class() {
    let policy = DenyAllPolicy;
    let requests = [
        read_request("/workspace/file"),
        CapabilityRequest::write_path(
            "/workspace/file",
            PathScope::PrimaryWorkspace,
            source("write_file"),
        ),
        process_request("cargo test"),
        CapabilityRequest::network(NetworkTarget::ToolManaged, source("web_search")),
        CapabilityRequest::skill("test", None, source("skill")),
        CapabilityRequest::instruction_discovery(
            "/workspace/AGENTS.md",
            PathScope::PrimaryWorkspace,
            CapabilitySource::PromptConstruction,
        ),
    ];
    assert_eq!(
        requests.each_ref().map(|request| request.kind()),
        [
            CapabilityKind::Read,
            CapabilityKind::Write,
            CapabilityKind::Process,
            CapabilityKind::Network,
            CapabilityKind::Skill,
            CapabilityKind::InstructionDiscovery,
        ]
    );
    for request in requests {
        assert!(matches!(
            policy.evaluate(&request),
            PolicyDecision::Deny { .. }
        ));
    }
}

#[test]
fn structured_process_context_never_requires_shell_parsing() {
    let request = process_request("printf '%s' '$TOKEN; rm -rf -- /'");
    let CapabilityOperation::ExecuteProcess(execution) = request.operation() else {
        panic!("expected process operation");
    };
    assert_eq!(
        execution.working_directory(),
        std::path::Path::new("/workspace")
    );
    assert_eq!(
        execution.invocation().executable_path(),
        std::path::Path::new("bash")
    );
    assert_eq!(execution.invocation().arguments(), ["-lc"]);
    assert_eq!(
        execution.invocation().shell_command(),
        Some("printf '%s' '$TOKEN; rm -rf -- /'")
    );
    assert_eq!(execution.environment(), &ProcessEnvironment::InheritAll);
    assert_eq!(execution.output_limits().max_output_bytes(), 4096);
    assert!(!format!("{request:?}").contains("$TOKEN"));
}

#[derive(Debug)]
struct RecordingApproval {
    requests: Mutex<Vec<ApprovalRequest>>,
    decision: ApprovalDecision,
}

impl ApprovalHandler for RecordingApproval {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(request);
            self.decision.clone()
        })
    }
}

#[tokio::test]
async fn remembered_approval_matches_only_the_exact_session_request() {
    let policy: Arc<dyn WorkspacePolicy> = Arc::new(
        ScopedWorkspacePolicy::new()
            .allow_processes()
            .require_process_approval(),
    );
    let approvals = Arc::new(RecordingApproval {
        requests: Mutex::new(Vec::new()),
        decision: ApprovalDecision::AllowForSession,
    });
    let erased: Arc<dyn ApprovalHandler> = approvals.clone();
    let remembered = Arc::new(SessionApprovals::default());
    let audit = Arc::new(ApprovalAuditLog::default());
    let request = process_request("cargo test");
    let call_id = crate::ToolCallId::from_string("approval-call").unwrap();

    let first = authorize_for_call(
        &policy,
        &erased,
        &remembered,
        &audit,
        request.clone(),
        Some(&call_id),
    )
    .await
    .unwrap();
    let repeated = authorize(&policy, &erased, &remembered, &audit, request)
        .await
        .unwrap();
    authorize(
        &policy,
        &erased,
        &remembered,
        &audit,
        process_request("cargo test --all"),
    )
    .await
    .unwrap();

    assert_eq!(first, super::AuthorizationOutcome::AllowedForSession);
    assert_eq!(
        approvals
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())[0]
            .tool_call_id(),
        Some(&call_id)
    );
    assert_eq!(
        repeated,
        super::AuthorizationOutcome::AllowedByRememberedApproval
    );
    assert_eq!(
        approvals
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len(),
        2
    );
    assert_eq!(
        audit
            .snapshot()
            .into_iter()
            .map(|record| record.decision())
            .collect::<Vec<_>>(),
        vec![
            ApprovalAuditDecision::AllowedForSession,
            ApprovalAuditDecision::AllowedByRememberedApproval,
            ApprovalAuditDecision::AllowedForSession,
        ]
    );
}

#[tokio::test]
async fn approval_receiver_skips_requests_cancelled_before_host_delivery() {
    let (handler, mut receiver) = approval_channel(NonZeroUsize::new(1).unwrap());
    let stale = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .request(ApprovalRequest::new(
                    process_request("stale"),
                    "approval required",
                ))
                .await
        }
    });
    handler.wait_until_full().await;
    stale.abort();
    assert!(stale.await.unwrap_err().is_cancelled());

    let live = tokio::spawn({
        let handler = handler.clone();
        async move {
            handler
                .request(ApprovalRequest::new(
                    process_request("live"),
                    "approval required",
                ))
                .await
        }
    });
    let mut pending = receiver.recv().await.unwrap();
    assert_eq!(pending.request().capability(), &process_request("live"));
    pending.respond(ApprovalDecision::AllowOnce).unwrap();
    assert_eq!(live.await.unwrap(), ApprovalDecision::AllowOnce);
}

#[tokio::test]
async fn approval_channel_handles_drop_and_exactly_once_response() {
    let (handler, mut receiver) = approval_channel(NonZeroUsize::new(1).unwrap());
    let request = ApprovalRequest::new(process_request("cargo test"), "approval required");
    let waiting = tokio::spawn({
        let handler = handler.clone();
        let request = request.clone();
        async move { handler.request(request).await }
    });
    let mut pending = receiver.recv().await.unwrap();
    assert!(pending.respond(ApprovalDecision::AllowOnce).is_ok());
    assert_eq!(
        pending.respond(ApprovalDecision::AllowOnce),
        Err(ApprovalDecision::AllowOnce)
    );
    assert_eq!(waiting.await.unwrap(), ApprovalDecision::AllowOnce);

    let waiting = tokio::spawn(async move { handler.request(request).await });
    drop(receiver.recv().await.unwrap());
    assert!(matches!(
        waiting.await.unwrap(),
        ApprovalDecision::Deny { reason } if reason.contains("dropped")
    ));
}

#[tokio::test]
async fn no_approval_handler_returns_typed_host_denial() {
    let policy: Arc<dyn WorkspacePolicy> = Arc::new(
        ScopedWorkspacePolicy::new()
            .allow_processes()
            .require_process_approval(),
    );
    let approvals: Arc<dyn ApprovalHandler> = Arc::new(DenyApprovals);
    let error = authorize(
        &policy,
        &approvals,
        &Arc::default(),
        &Arc::default(),
        process_request("cargo test"),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), super::AuthorizationDenialKind::Host);
    assert_eq!(error.capability(), CapabilityKind::Process);
}

#[derive(Debug)]
struct HoldingApproval {
    prompts: Mutex<u32>,
    release: tokio::sync::Notify,
}

impl ApprovalHandler for HoldingApproval {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            let count = {
                let mut prompts = self
                    .prompts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *prompts += 1;
                *prompts
            };
            // Hold the first prompt open so a concurrent identical request must
            // wait on the in-flight gate rather than prompting a second time.
            if count == 1 {
                self.release.notified().await;
            }
            ApprovalDecision::AllowForSession
        })
    }
}

#[tokio::test]
async fn concurrent_identical_requests_prompt_the_host_once() {
    let policy: Arc<dyn WorkspacePolicy> = Arc::new(
        ScopedWorkspacePolicy::new()
            .allow_processes()
            .require_process_approval(),
    );
    let approvals = Arc::new(HoldingApproval {
        prompts: Mutex::new(0),
        release: tokio::sync::Notify::new(),
    });
    let erased: Arc<dyn ApprovalHandler> = approvals.clone();
    let remembered = Arc::new(SessionApprovals::default());
    let audit = Arc::new(ApprovalAuditLog::default());
    let request = process_request("cargo test");

    let (first, second, ()) = tokio::join!(
        authorize_for_call(&policy, &erased, &remembered, &audit, request.clone(), None),
        authorize_for_call(&policy, &erased, &remembered, &audit, request.clone(), None),
        async {
            while *approvals
                .prompts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                == 0
            {
                tokio::task::yield_now().await;
            }
            // The first request is now parked on the prompt; let the second park
            // on the gate before releasing.
            tokio::task::yield_now().await;
            approvals.release.notify_waiters();
        },
    );

    assert_eq!(
        first.unwrap(),
        super::AuthorizationOutcome::AllowedForSession
    );
    assert_eq!(
        second.unwrap(),
        super::AuthorizationOutcome::AllowedByRememberedApproval
    );
    assert_eq!(
        *approvals
            .prompts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
        1
    );
}
