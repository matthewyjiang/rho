use std::sync::Mutex;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{
    authorize, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CapabilityRequest, DenyAllPolicy, DenyApprovals, PolicyDecision, ScopedWorkspacePolicy,
    Workspace, WorkspacePolicy,
};

#[test]
fn workspace_requires_absolute_root_and_rejects_lexical_escapes() {
    assert!(Workspace::new("relative").is_err());
    let root = TempDir::new().unwrap();
    let workspace = Workspace::new(root.path()).unwrap();

    assert_eq!(
        workspace.resolve("src/lib.rs").unwrap(),
        workspace.root().join("src/lib.rs")
    );
    assert!(workspace.resolve("../secret").is_err());
    assert!(workspace
        .resolve(root.path().with_extension("outside"))
        .is_err());
    assert!(Workspace::new(root.path().join("missing")).is_err());
}

#[cfg(unix)]
#[test]
fn existing_path_resolution_rejects_symlinks_outside_workspace() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret"), "secret").unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let workspace = Workspace::new(root.path()).unwrap();

    assert!(workspace.resolve_existing("escape/secret").is_err());
}

#[test]
fn default_policy_denies_files_processes_and_network() {
    let policy = DenyAllPolicy;
    let requests = [
        CapabilityRequest::ReadPath {
            path: "/workspace/file".into(),
        },
        CapabilityRequest::WritePath {
            path: "/workspace/file".into(),
        },
        CapabilityRequest::ExecuteProcess {
            program: "cargo".into(),
            arguments: vec!["test".into()],
        },
        CapabilityRequest::NetworkAccess {
            url: "https://example.com".into(),
        },
    ];

    for request in requests {
        assert!(matches!(
            policy.evaluate(&request),
            PolicyDecision::Deny { .. }
        ));
    }
}

#[test]
fn scoped_policy_grants_only_named_capabilities_and_hosts() {
    let policy = ScopedWorkspacePolicy::new()
        .allow_read_paths()
        .allow_network_host("example.com");

    assert_eq!(
        policy.evaluate(&CapabilityRequest::ReadPath {
            path: "/workspace/file".into(),
        }),
        PolicyDecision::Allow
    );
    assert_eq!(
        policy.evaluate(&CapabilityRequest::NetworkAccess {
            url: "https://example.com/path".into(),
        }),
        PolicyDecision::Allow
    );
    assert!(matches!(
        policy.evaluate(&CapabilityRequest::NetworkAccess {
            url: "https://other.example/path".into(),
        }),
        PolicyDecision::Deny { .. }
    ));
    assert!(matches!(
        policy.evaluate(&CapabilityRequest::ExecuteProcess {
            program: "cargo".into(),
            arguments: Vec::new(),
        }),
        PolicyDecision::Deny { .. }
    ));
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
async fn required_approval_is_explicit_and_deny_by_default() {
    let policy: std::sync::Arc<dyn WorkspacePolicy> = std::sync::Arc::new(
        ScopedWorkspacePolicy::new()
            .allow_processes()
            .require_process_approval(),
    );
    let request = CapabilityRequest::ExecuteProcess {
        program: "cargo".into(),
        arguments: vec!["test".into()],
    };

    let deny_approvals: std::sync::Arc<dyn ApprovalHandler> = std::sync::Arc::new(DenyApprovals);
    assert!(authorize(&policy, &deny_approvals, request.clone())
        .await
        .is_err());

    let approvals = std::sync::Arc::new(RecordingApproval {
        requests: Mutex::new(Vec::new()),
        decision: ApprovalDecision::AllowOnce,
    });
    let erased: std::sync::Arc<dyn ApprovalHandler> = approvals.clone();
    authorize(&policy, &erased, request.clone()).await.unwrap();

    assert_eq!(
        approvals
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        [ApprovalRequest {
            capability: request,
            reason: "host approval is required".into(),
        }]
    );
}
