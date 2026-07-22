#[cfg(unix)]
mod unix {
    use std::sync::{Arc, Mutex};

    use pretty_assertions::assert_eq;
    use rho_sdk::{
        model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
        provider::{ScriptedProvider, ScriptedTurn},
        ApprovalAuditDecision, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
        CapabilityKind, CapabilityOperation, CapabilitySource, ExecutableSelection,
        ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits, Rho,
        RunEvent, ScopedWorkspacePolicy, SessionOptions, ToolCompletion, UserInput, Workspace,
    };
    use serde_json::json;

    use super::super::{SdkShellTool, ShellToolOptions};

    #[derive(Debug)]
    struct ApproveAndRecord {
        requests: Mutex<Vec<ApprovalRequest>>,
    }

    impl ApprovalHandler for ApproveAndRecord {
        fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
            Box::pin(async move {
                self.requests
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(request);
                ApprovalDecision::AllowOnce
            })
        }
    }

    #[tokio::test]
    async fn approved_process_facts_are_the_command_that_executes() {
        let root = tempfile::tempdir().unwrap();
        let command = "printf '%s' '$TOKEN; && | $(touch must-not-exist)' > exact-output.txt";
        let provider = ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: "shell-1".into(),
                        name: "bash".into(),
                        arguments: json!({"command": command, "timeout_seconds": 9}),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ],
        );
        let approvals = Arc::new(ApproveAndRecord {
            requests: Mutex::new(Vec::new()),
        });
        let runtime = Rho::builder()
            .provider(provider)
            .workspace(Workspace::new(root.path()).unwrap())
            .workspace_policy(
                ScopedWorkspacePolicy::new()
                    .allow_processes()
                    .require_process_approval(),
            )
            .approval_handler_shared(approvals.clone())
            .tool(SdkShellTool::bash(
                ShellToolOptions::new().max_output_bytes(777),
            ))
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        let mut run = session.start(UserInput::text("run it")).await.unwrap();
        let mut completion = None;
        while let Some(event) = run.next_event().await {
            if let RunEvent::ToolFinished { result, .. } = event {
                completion = Some(result);
            }
        }
        run.outcome().await.unwrap();

        assert!(matches!(completion, Some(ToolCompletion::Success(_))));
        assert_eq!(
            std::fs::read_to_string(root.path().join("exact-output.txt")).unwrap(),
            "$TOKEN; && | $(touch must-not-exist)"
        );
        assert!(!root.path().join("must-not-exist").exists());

        let expected = ProcessExecution::new(
            root.path().canonicalize().unwrap(),
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
            ProcessEnvironment::InheritAll,
            ProcessOutputLimits::new(777, Some(std::time::Duration::from_secs(9))),
        );
        let requests = approvals
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].capability().source(),
            &CapabilitySource::built_in_tool("bash")
        );
        let CapabilityOperation::ExecuteProcess(approved) = requests[0].capability().operation()
        else {
            panic!("shell approval must contain typed process facts");
        };
        assert_eq!(approved, &expected);
        assert_eq!(
            approved.invocation().executable_selection(),
            ExecutableSelection::SearchPath
        );
        drop(requests);

        assert_eq!(
            runtime
                .diagnostics()
                .approval_audit()
                .iter()
                .map(|record| (record.capability(), record.decision()))
                .collect::<Vec<_>>(),
            [(CapabilityKind::Process, ApprovalAuditDecision::AllowedOnce)]
        );
    }
}

#[cfg(windows)]
mod windows {
    use pretty_assertions::assert_eq;
    use rho_sdk::ProcessInvocation;

    use super::super::ShellKind;

    #[test]
    fn powershell_plan_authorizes_the_wrapped_command_that_executes() {
        let command = "Write-Output 'exact'";
        let invocation = ShellKind::PowerShell.invocation(command.into());
        let ProcessInvocation::Shell {
            executable,
            arguments,
            command: planned_command,
            ..
        } = invocation
        else {
            panic!("PowerShell must use a shell process plan");
        };
        assert_eq!(executable, std::path::Path::new("powershell.exe"));
        assert_eq!(arguments, ["-NoProfile", "-NonInteractive", "-Command"]);
        assert_eq!(planned_command, crate::powershell::wrapped_command(command));
    }
}
