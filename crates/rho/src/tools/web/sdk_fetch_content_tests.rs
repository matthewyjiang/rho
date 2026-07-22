#[cfg(unix)]
use std::path::PathBuf;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::ToolErrorKind,
    CapabilityOperation, CapabilityRequest, ExecutableSelection, NetworkTarget, PathScope,
    PolicyDecision, Rho, RunEvent, ScopedWorkspacePolicy, SessionOptions, ToolCompletion,
    UserInput, Workspace, WorkspacePolicy,
};
use serde_json::json;

use super::*;

#[derive(Clone)]
struct RecordingPolicy {
    inner: ScopedWorkspacePolicy,
    requests: Arc<Mutex<Vec<CapabilityRequest>>>,
}

impl RecordingPolicy {
    fn new(inner: ScopedWorkspacePolicy) -> Self {
        Self {
            inner,
            requests: Arc::default(),
        }
    }

    fn requests(&self) -> Vec<CapabilityRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl WorkspacePolicy for RecordingPolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request.clone());
        self.inner.evaluate(request)
    }
}

async fn run_fetch(
    workspace: Workspace,
    policy: impl WorkspacePolicy + 'static,
    arguments: serde_json::Value,
) -> ToolCompletion {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "fetch-1".into(),
                    name: FETCH_CONTENT_TOOL.into(),
                    arguments,
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .workspace(workspace)
        .workspace_policy(policy)
        .tool(SdkFetchContent::new(
            12_000,
            super::super::guard::NetworkAccess::AllowPrivate,
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let mut run = session.start(UserInput::text("fetch it")).await.unwrap();
    let mut completion = None;
    while let Some(event) = run.next_event().await {
        if let RunEvent::ToolFinished { result, .. } = event {
            completion = Some(result);
        }
    }
    run.outcome().await.unwrap();
    completion.unwrap()
}

#[tokio::test]
async fn local_target_requires_read_instead_of_tool_managed_network() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("note.txt");
    std::fs::write(&file, "workspace secret").unwrap();
    let policy =
        RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_network_tool(FETCH_CONTENT_TOOL));

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({"urls": ["note.txt"]}),
    )
    .await;

    let ToolCompletion::Failure(failure) = completion else {
        panic!("read-denied local fetch should fail");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    let requests = policy.requests();
    assert_eq!(requests.len(), 1);
    let CapabilityOperation::ReadPath { path, scope } = requests[0].operation() else {
        panic!("local target must request a read path");
    };
    assert_eq!(path, &file.canonicalize().unwrap());
    assert_eq!(scope, &PathScope::PrimaryWorkspace);
}

#[tokio::test]
async fn local_target_reads_only_after_workspace_authorization() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "workspace content").unwrap();
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_read_paths());

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({"urls": ["note.txt"]}),
    )
    .await;

    let ToolCompletion::Success(output) = completion else {
        panic!("authorized local fetch should succeed");
    };
    assert!(output.content().contains("workspace content"));
    assert!(matches!(
        policy.requests()[0].operation(),
        CapabilityOperation::ReadPath { .. }
    ));
}

#[tokio::test]
async fn local_target_outside_workspace_requires_a_granted_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("secret.txt");
    std::fs::write(&file, "outside content").unwrap();
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_read_paths());

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({"urls": [file]}),
    )
    .await;

    let ToolCompletion::Failure(failure) = completion else {
        panic!("ungranted outside path should fail");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    assert!(policy.requests().is_empty());
}

#[tokio::test]
async fn granted_root_still_requires_explicit_outside_workspace_policy() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("granted.txt");
    std::fs::write(&file, "granted content").unwrap();
    let workspace = Workspace::new(root.path())
        .unwrap()
        .with_granted_root(outside.path())
        .unwrap();
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_read_paths());

    let completion = run_fetch(
        workspace.clone(),
        policy.clone(),
        json!({"urls": [file.clone()]}),
    )
    .await;
    let ToolCompletion::Failure(failure) = completion else {
        panic!("outside-workspace policy should remain independent");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    let requests = policy.requests();
    let CapabilityOperation::ReadPath { scope, .. } = requests[0].operation() else {
        panic!("granted local target must request a read path");
    };
    assert!(matches!(scope, PathScope::GrantedRoot { .. }));

    let allowed = run_fetch(
        workspace,
        ScopedWorkspacePolicy::new()
            .allow_read_paths()
            .allow_outside_workspace_paths(),
        json!({"urls": [file]}),
    )
    .await;
    assert!(matches!(allowed, ToolCompletion::Success(_)));
}

#[tokio::test]
async fn http_target_requests_the_exact_url() {
    let root = tempfile::tempdir().unwrap();
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new());
    let url = "https://example.com/articles/one?view=full";

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({"urls": [url]}),
    )
    .await;

    assert!(matches!(completion, ToolCompletion::Failure(_)));
    let requests = policy.requests();
    assert_eq!(requests.len(), 1);
    let CapabilityOperation::NetworkAccess(NetworkTarget::Url(actual)) = requests[0].operation()
    else {
        panic!("HTTP target must request an exact URL");
    };
    assert_eq!(actual, url);
}

#[tokio::test]
async fn authorized_http_target_executes_the_authorized_url() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "authorized HTTP fetch did not connect"
                    );
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("test HTTP server failed: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            let bytes_read = reader.read_line(&mut line).unwrap();
            assert_ne!(bytes_read, 0, "HTTP request ended before its headers");
            if line == "\r\n" {
                break;
            }
        }
        drop(reader);
        let body = "<html><title>Local Test</title><p>authorized response</p></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_network_host("127.0.0.1"));
    let root = tempfile::tempdir().unwrap();
    let url = format!("http://{address}/article");

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({"urls": [&url]}),
    )
    .await;
    server.join().unwrap();

    let ToolCompletion::Success(output) = completion else {
        panic!("authorized HTTP fetch should succeed");
    };
    assert!(output.content().contains("authorized response"));
    let requests = policy.requests();
    let CapabilityOperation::NetworkAccess(NetworkTarget::Url(actual)) = requests[0].operation()
    else {
        panic!("HTTP target must request an exact URL");
    };
    assert_eq!(actual, &url);
}

#[tokio::test]
async fn github_api_target_authorizes_the_executed_api_url() {
    let root = tempfile::tempdir().unwrap();
    let policy = RecordingPolicy::new(ScopedWorkspacePolicy::new());

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({"urls": ["https://github.com/acme/project/blob/main/README.md"]}),
    )
    .await;

    assert!(matches!(completion, ToolCompletion::Failure(_)));
    let requests = policy.requests();
    let CapabilityOperation::NetworkAccess(NetworkTarget::Url(actual)) = requests[0].operation()
    else {
        panic!("GitHub API target must request an exact URL");
    };
    assert_eq!(
        actual,
        "https://api.github.com/repos/acme/project/contents/README.md?ref=main"
    );
}

#[tokio::test]
async fn force_clone_authorizes_exact_network_and_process_plan_before_execution() {
    let root = tempfile::tempdir().unwrap();
    let workspace_root = root.path().canonicalize().unwrap();
    let policy =
        RecordingPolicy::new(ScopedWorkspacePolicy::new().allow_network_host("github.com"));

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        policy.clone(),
        json!({
            "urls": ["https://github.com/acme/project/tree/main/src"],
            "forceClone": true
        }),
    )
    .await;

    let ToolCompletion::Failure(failure) = completion else {
        panic!("process-denied force clone should fail before execution");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
    let requests = policy.requests();
    assert_eq!(requests.len(), 2);
    let CapabilityOperation::NetworkAccess(NetworkTarget::Url(network_url)) =
        requests[0].operation()
    else {
        panic!("force clone must authorize its clone URL");
    };
    assert_eq!(network_url, "https://github.com/acme/project.git");
    let CapabilityOperation::ExecuteProcess(process) = requests[1].operation() else {
        panic!("force clone must authorize git execution");
    };
    assert_eq!(process.working_directory(), workspace_root);
    assert_eq!(
        process.invocation().executable_path(),
        std::path::Path::new("git")
    );
    assert_eq!(
        process.invocation().executable_selection(),
        ExecutableSelection::SearchPath
    );
    let arguments = process.invocation().arguments();
    assert_eq!(arguments.len(), 5);
    assert_eq!(
        arguments[..4],
        [
            "clone",
            "--depth",
            "1",
            "https://github.com/acme/project.git"
        ]
    );
    let clone_path = std::path::Path::new(&arguments[4]);
    assert!(!clone_path.exists());
    assert_eq!(
        clone_path.file_name().and_then(|name| name.to_str()),
        Some("0")
    );
}

#[cfg(unix)]
#[derive(Clone)]
struct ReplaceAuthorizedFile {
    path: PathBuf,
    outside: PathBuf,
}

#[cfg(unix)]
impl WorkspacePolicy for ReplaceAuthorizedFile {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        if matches!(request.operation(), CapabilityOperation::ReadPath { .. }) {
            std::fs::remove_file(&self.path).unwrap();
            std::os::unix::fs::symlink(&self.outside, &self.path).unwrap();
        }
        PolicyDecision::Allow
    }
}

#[cfg(unix)]
#[tokio::test]
async fn local_target_is_revalidated_after_authorization() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = root.path().join("note.txt");
    let outside_path = outside.path().join("secret.txt");
    std::fs::write(&path, "safe").unwrap();
    std::fs::write(&outside_path, "secret").unwrap();

    let completion = run_fetch(
        Workspace::new(root.path()).unwrap(),
        ReplaceAuthorizedFile {
            path,
            outside: outside_path,
        },
        json!({"urls": ["note.txt"]}),
    )
    .await;

    let ToolCompletion::Failure(failure) = completion else {
        panic!("changed path should fail revalidation");
    };
    assert_eq!(failure.kind(), ToolErrorKind::PolicyDenied);
}
