use clap::Parser;

use super::*;

#[test]
fn parses_new_provider_auth_modes() {
    for auth in rho_providers::auth_profiles() {
        let cli = Cli::try_parse_from(["rho", "--auth", auth]).unwrap();
        assert_eq!(cli.auth.as_deref(), Some(*auth));
    }
}

#[test]
fn rejects_unknown_auth_profiles() {
    let error = Cli::try_parse_from(["rho", "--auth", "not-a-real-auth"]).unwrap_err();
    assert!(error
        .to_string()
        .contains("invalid value 'not-a-real-auth'"));
    assert!(error.to_string().contains("ollama-cloud-api-key"));
}

// Covers: `rho acp` parses as the ACP stdio subcommand.
// Owner: CLI parser.
#[test]
fn parses_acp_subcommand() {
    let cli = Cli::try_parse_from(["rho", "acp"]).unwrap();

    assert!(matches!(cli.command, Some(Command::Acp {})));
}

#[test]
fn parses_attach_subcommand() {
    let cli = Cli::try_parse_from(["rho", "attach", "abc123"]).unwrap();

    assert!(matches!(
        cli.command,
        Some(Command::Attach { id }) if id == "abc123"
    ));
}

#[test]
fn attach_requires_an_id() {
    let error = Cli::try_parse_from(["rho", "attach"]).unwrap_err();

    assert!(error.to_string().contains("<ID>"));
}

#[test]
fn parses_sessions_list_and_rm() {
    let list = Cli::try_parse_from(["rho", "sessions", "list"]).unwrap();
    assert!(matches!(
        list.command,
        Some(Command::Sessions {
            command: SessionsCommand::List {
                all_projects: false,
                search: None,
                limit: None,
                json: false,
            }
        })
    ));

    let all = Cli::try_parse_from([
        "rho",
        "sessions",
        "list",
        "--all-projects",
        "--search",
        "login",
        "--limit",
        "5",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        all.command,
        Some(Command::Sessions {
            command: SessionsCommand::List {
                all_projects: true,
                search: Some(query),
                limit: Some(limit),
                json: true,
            }
        }) if query == "login" && limit.get() == 5
    ));

    let export = Cli::try_parse_from([
        "rho", "sessions", "export", "abc123", "--output", "notes.md", "--format", "markdown",
        "--force",
    ])
    .unwrap();
    assert!(matches!(
        export.command,
        Some(Command::Sessions {
            command: SessionsCommand::Export {
                id_prefix,
                output: Some(path),
                format: Some(crate::export::ExportFormat::Markdown),
                force: true,
            }
        }) if id_prefix == "abc123" && path.ends_with("notes.md")
    ));

    let rm = Cli::try_parse_from(["rho", "sessions", "rm", "abc", "def", "--force", "-y"]).unwrap();
    assert!(matches!(
        rm.command,
        Some(Command::Sessions {
            command: SessionsCommand::Rm {
                ids,
                force: true,
                yes: true
            }
        }) if ids == ["abc", "def"]
    ));

    let cleanup = Cli::try_parse_from(["rho", "sessions", "cleanup", "--force", "--yes"]).unwrap();
    assert!(matches!(
        cleanup.command,
        Some(Command::Sessions {
            command: SessionsCommand::Cleanup {
                force: true,
                yes: true
            }
        })
    ));

    let rename = Cli::try_parse_from([
        "rho", "sessions", "rename", "abc123", "new", "session", "title",
    ])
    .unwrap();
    assert!(matches!(
        rename.command,
        Some(Command::Sessions {
            command: SessionsCommand::Rename { id_prefix, title }
        }) if id_prefix == "abc123" && title == ["new", "session", "title"]
    ));
}

#[test]
fn parses_permission_mode_override() {
    for (flag, expected) in [
        ("bypass", crate::permission::PermissionMode::Bypass),
        ("auto", crate::permission::PermissionMode::Auto),
        ("allow_edits", crate::permission::PermissionMode::AllowEdits),
        ("plan", crate::permission::PermissionMode::Plan),
        ("supervised", crate::permission::PermissionMode::Supervised),
    ] {
        let cli = Cli::try_parse_from(["rho", "--permission-mode", flag]).unwrap();
        assert_eq!(cli.permission_mode, Some(expected));
    }

    let run = Cli::try_parse_from(["rho", "--permission-mode", "auto", "run", "ship it"]).unwrap();
    assert_eq!(
        run.permission_mode,
        Some(crate::permission::PermissionMode::Auto)
    );
}

#[test]
fn rejects_unknown_permission_mode() {
    let error = Cli::try_parse_from(["rho", "--permission-mode", "paranoid"]).unwrap_err();
    assert!(error.to_string().contains("paranoid"));
    assert!(error
        .to_string()
        .contains("bypass, auto, allow_edits, plan, or supervised"));
}

#[test]
fn agent_selection_is_global() {
    let root = Cli::try_parse_from(["rho", "--agent", "reviewer"]).unwrap();
    assert_eq!(root.agent.as_deref(), Some("reviewer"));

    let run = Cli::try_parse_from(["rho", "run", "--agent", "worker", "ship it"]).unwrap();
    assert_eq!(run.agent.as_deref(), Some("worker"));
}

#[test]
fn parses_credential_store_commands() {
    use rho_providers::credentials::CredentialStoreBackend;

    let probe = Cli::try_parse_from(["rho", "credential-store", "probe", "os"]).unwrap();
    assert!(matches!(
        probe.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Probe { backend }
        }) if backend == CredentialStoreBackend::Os
    ));

    let probe_default = Cli::try_parse_from(["rho", "credential-store", "probe"]).unwrap();
    assert!(matches!(
        probe_default.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Probe { backend }
        }) if backend == CredentialStoreBackend::Os
    ));

    let probe_auto = Cli::try_parse_from(["rho", "credential-store", "probe", "auto"]).unwrap();
    assert!(matches!(
        probe_auto.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Probe { backend }
        }) if backend == CredentialStoreBackend::Os
    ));

    let set = Cli::try_parse_from(["rho", "credential-store", "set", "file"]).unwrap();
    assert!(matches!(
        set.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Set { backend }
        }) if backend == CredentialStoreBackend::File
    ));

    let status = Cli::try_parse_from(["rho", "credential-store", "status"]).unwrap();
    assert!(matches!(
        status.command,
        Some(Command::CredentialStore {
            command: CredentialStoreCommand::Status
        })
    ));
}

#[test]
fn rejects_unknown_credential_store_backend() {
    assert!(Cli::try_parse_from(["rho", "credential-store", "probe", "sqlite"]).is_err());
    assert!(Cli::try_parse_from(["rho", "credential-store", "set", "sqlite"]).is_err());
}

#[test]
fn parses_structured_output_and_execution_bounds() {
    let cli = Cli::try_parse_from([
        "rho",
        "run",
        "--output",
        "jsonl",
        "--max-steps",
        "12",
        "--timeout",
        "20m",
        "ship it",
    ])
    .unwrap();

    assert!(matches!(
        cli.command,
        Some(Command::Run {
            output: OutputFormat::Jsonl,
            max_steps: Some(max_steps),
            timeout: Some(timeout),
            ..
        }) if max_steps.get() == 12 && timeout == std::time::Duration::from_secs(1_200)
    ));
}

#[test]
fn rejects_zero_steps_and_invalid_durations() {
    for arguments in [
        ["rho", "run", "--max-steps", "0"],
        ["rho", "run", "--timeout", "soon"],
    ] {
        assert!(Cli::try_parse_from(arguments).is_err());
    }
}

// Covers: workflow command flags could drift from the documented shell contract.
// Owner: CLI parser.
#[test]
fn parses_all_workflow_commands_and_output_modes() {
    let list = Cli::try_parse_from([
        "rho", "workflow", "list", "--runs", "--limit", "3", "--json",
    ])
    .unwrap();
    assert!(matches!(
        list.command,
        Some(Command::Workflow {
            command: WorkflowCommand::List {
                plans: false,
                runs: true,
                limit: Some(limit),
                json: true,
            }
        }) if limit.get() == 3
    ));

    let validate = Cli::try_parse_from([
        "rho",
        "workflow",
        "validate",
        "review.star",
        "--input",
        "target=\"src\"",
    ])
    .unwrap();
    assert!(matches!(
        validate.command,
        Some(Command::Workflow {
            command: WorkflowCommand::Validate { file, input }
        }) if file.as_path() == std::path::Path::new("review.star") && input == ["target=\"src\""]
    ));

    let plan = Cli::try_parse_from(["rho", "workflow", "plan", "review.star", "--output", "json"])
        .unwrap();
    assert!(matches!(
        plan.command,
        Some(Command::Workflow {
            command: WorkflowCommand::Plan {
                output: WorkflowDocumentFormat::Json,
                ..
            }
        })
    ));

    let run = Cli::try_parse_from([
        "rho",
        "workflow",
        "run",
        "plan-prefix",
        "--yes",
        "--output",
        "jsonl",
    ])
    .unwrap();
    assert!(matches!(
        run.command,
        Some(Command::Workflow {
            command: WorkflowCommand::Run {
                plan_id,
                yes: true,
                output: Some(WorkflowRunFormat::Jsonl)
            }
        }) if plan_id == "plan-prefix"
    ));

    let status = Cli::try_parse_from([
        "rho",
        "workflow",
        "status",
        "run-prefix",
        "--output",
        "json",
    ])
    .unwrap();
    assert!(matches!(
        status.command,
        Some(Command::Workflow {
            command: WorkflowCommand::Status {
                output: WorkflowDocumentFormat::Json,
                ..
            }
        })
    ));

    let cancel = Cli::try_parse_from(["rho", "workflow", "cancel", "run-prefix"]).unwrap();
    assert!(matches!(
        cancel.command,
        Some(Command::Workflow {
            command: WorkflowCommand::Cancel { run_id }
        }) if run_id == "run-prefix"
    ));

    let resume = Cli::try_parse_from([
        "rho",
        "workflow",
        "resume",
        "run-prefix",
        "--yes",
        "--output",
        "text",
    ])
    .unwrap();
    assert!(matches!(
        resume.command,
        Some(Command::Workflow {
            command: WorkflowCommand::Resume {
                yes: true,
                output: Some(WorkflowRunFormat::Text),
                ..
            }
        })
    ));
}

#[test]
fn workflow_commands_reject_wrong_output_modes() {
    assert!(Cli::try_parse_from([
        "rho",
        "workflow",
        "plan",
        "review.star",
        "--output",
        "jsonl"
    ])
    .is_err());
    assert!(
        Cli::try_parse_from(["rho", "workflow", "run", "plan-id", "--output", "json"]).is_err()
    );
}

// Covers: the supervised planner worker needs a dedicated hidden argv entry, not a
// costume validate path, so public validate stays ordinary and the worker stays reachable.
// Owner: CLI parser.
#[test]
fn parses_hidden_workflow_planner_worker_command() {
    let cli = Cli::try_parse_from(["rho", WORKFLOW_PLANNER_WORKER_COMMAND]).unwrap();
    assert!(matches!(cli.command, Some(Command::WorkflowPlannerWorker)));
    assert!(Cli::try_parse_from(["rho", "workflow", "validate", "worker.star"]).is_ok());
}
