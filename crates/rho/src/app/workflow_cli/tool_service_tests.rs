use std::{collections::BTreeMap, path::Path};

use pretty_assertions::assert_eq;
use rho_sdk::{CapabilityKind, CapabilityOperation, PathScope};
#[cfg(any(unix, windows))]
use sha2::Digest as _;

use super::{
    agent_catalog_roots_for, executable_candidates_in, AppWorkflowToolService, WorkflowToolRequest,
};

fn service() -> AppWorkflowToolService {
    AppWorkflowToolService {
        cwd: "/workspace".into(),
        config_path: None,
    }
}

// Covers: each model workflow action must declare its durable and process
// authority before service dispatch, including explicit recovery authority.
// Owner: workflow application tool adapter.
#[test]
fn action_preparation_declares_exact_capabilities() {
    let cases = [
        (
            WorkflowToolRequest::Validate {
                file: "flow.star".into(),
                inputs: BTreeMap::new(),
            },
            vec![
                CapabilityKind::Read,
                CapabilityKind::Read,
                CapabilityKind::Read,
                CapabilityKind::Read,
                CapabilityKind::Process,
            ],
        ),
        (
            WorkflowToolRequest::Plan {
                file: "flow.star".into(),
                inputs: BTreeMap::new(),
            },
            vec![
                CapabilityKind::Read,
                CapabilityKind::Read,
                CapabilityKind::Read,
                CapabilityKind::Read,
                CapabilityKind::Process,
                CapabilityKind::Write,
            ],
        ),
        (
            WorkflowToolRequest::Run {
                plan_id: "00000000-0000-0000-0000-000000000001".into(),
            },
            vec![CapabilityKind::Read, CapabilityKind::Write],
        ),
        (
            WorkflowToolRequest::Status {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
            },
            vec![CapabilityKind::Read],
        ),
        (
            WorkflowToolRequest::Cancel {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
            },
            vec![CapabilityKind::Read, CapabilityKind::Write],
        ),
        (
            WorkflowToolRequest::Resume {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
                recover_uncertain: true,
            },
            vec![CapabilityKind::Read, CapabilityKind::Write],
        ),
    ];
    for (request, expected) in cases {
        let capabilities = service()
            .capabilities_for_paths(
                &request,
                Path::new("/rho"),
                Some(Path::new("/home/test")),
                Path::new("/bin/rho"),
            )
            .unwrap();
        assert_eq!(
            capabilities
                .iter()
                .map(|request| request.kind())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(capabilities.iter().all(
            |request| request.source() == &rho_sdk::CapabilitySource::built_in_tool("workflow")
        ));
    }
}

// Covers: action preparation must bind approval to the exact durable target
// and exact planner process facts, not only to a broad capability class.
// Owner: workflow application tool adapter.
#[test]
fn preparation_keeps_exact_durable_and_process_facts() {
    let plan = service()
        .capabilities_for_paths(
            &WorkflowToolRequest::Plan {
                file: "flow.star".into(),
                inputs: BTreeMap::new(),
            },
            Path::new("/rho"),
            Some(Path::new("/home/test")),
            Path::new("/bin/rho"),
        )
        .unwrap();
    assert!(matches!(
        plan[0].operation(),
        CapabilityOperation::ReadPath { path, scope }
            if path == Path::new("/workspace/flow.star") && *scope == PathScope::PrimaryWorkspace
    ));
    assert!(matches!(
        plan[1].operation(),
        CapabilityOperation::ReadPath { path, scope }
            if path == Path::new("/rho/config.toml")
                && *scope == PathScope::UnrestrictedFilesystem
    ));
    assert!(matches!(
        plan[2].operation(),
        CapabilityOperation::ReadPath { path, scope }
            if path == Path::new("/home/test/.agents/agents")
                && *scope == PathScope::UnrestrictedFilesystem
    ));
    assert!(matches!(
        plan[3].operation(),
        CapabilityOperation::ReadPath { path, scope }
            if path == Path::new("/home/test/.rho/agents")
                && *scope == PathScope::UnrestrictedFilesystem
    ));
    assert!(matches!(
        plan[4].operation(),
        CapabilityOperation::ExecuteProcess(process)
            if process.invocation().executable_path() == Path::new("/bin/rho")
                && process.invocation().arguments()
                    == [crate::cli::WORKFLOW_PLANNER_WORKER_COMMAND]
    ));
    assert!(matches!(
        plan[5].operation(),
        CapabilityOperation::WritePath { path, scope }
            if path == Path::new("/rho/workflows/plans")
                && *scope == PathScope::UnrestrictedFilesystem
    ));

    let recovery = service()
        .capabilities_for_paths(
            &WorkflowToolRequest::Resume {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
                recover_uncertain: true,
            },
            Path::new("/rho"),
            Some(Path::new("/home/test")),
            Path::new("/bin/rho"),
        )
        .unwrap();
    assert!(matches!(
        recovery[1].operation(),
        CapabilityOperation::WritePath { path, .. }
            if path == Path::new("/rho/workflows/runs/00000000-0000-0000-0000-000000000002")
    ));
    assert!(service()
        .capabilities_for_paths(
            &WorkflowToolRequest::Validate {
                file: "../outside.star".into(),
                inputs: BTreeMap::new(),
            },
            Path::new("/rho"),
            Some(Path::new("/home/test")),
            Path::new("/bin/rho"),
        )
        .is_err());
}

// Covers: catalog and PATH discovery must turn each possible read into an
// exact path before model-tool planning resolves an agent or executable.
// Owner: workflow application tool capability preparation.
#[test]
fn discovery_capability_paths_are_exact() {
    assert_eq!(
        agent_catalog_roots_for(
            Path::new("/workspace/project"),
            Some(Path::new("/home/test")),
            true,
        ),
        vec![
            Path::new("/home/test/.agents/agents").to_path_buf(),
            Path::new("/home/test/.rho/agents").to_path_buf(),
            Path::new("/workspace/project/.agents/agents").to_path_buf(),
        ]
    );
    assert_eq!(
        executable_candidates_in(
            "cargo",
            [
                Path::new("/usr/bin").to_path_buf(),
                Path::new("/bin").to_path_buf()
            ],
        ),
        if cfg!(windows) {
            vec![
                Path::new("/usr/bin/cargo.exe").to_path_buf(),
                Path::new("/bin/cargo.exe").to_path_buf(),
            ]
        } else {
            vec![
                Path::new("/usr/bin/cargo").to_path_buf(),
                Path::new("/bin/cargo").to_path_buf(),
            ]
        }
    );
}

#[cfg(any(unix, windows))]
fn replace_opened_file(path: &Path, replacement: &str) {
    std::fs::rename(path, path.with_extension("authorized")).unwrap();
    std::fs::write(path, replacement).unwrap();
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(windows)]
fn make_executable(_path: &Path) {}

// Covers: replacing config after its identity check must not change parsed model policy.
// Owner: workflow dynamic capability adapter.
#[cfg(any(unix, windows))]
#[test]
fn config_parse_uses_the_authorized_open_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "provider = 'openai'\nmodel = 'authorized'\n").unwrap();
    let opened = crate::workflow::open_verified_file(&path, false).unwrap();

    replace_opened_file(&path, "provider = 'openai'\nmodel = 'replacement'\n");
    let text = crate::workflow::read_opened_utf8(opened).unwrap();
    let config = crate::config::Config::parse_settings(&text).unwrap();

    assert_eq!(config.model, "authorized");
}

// Covers: replacing a workflow source after approval must not change the
// bounded bytes sent to the planner.
// Owner: workflow dynamic capability adapter.
#[cfg(any(unix, windows))]
#[test]
fn workflow_source_read_uses_the_authorized_open_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workflow.star");
    std::fs::write(&path, "authorized source").unwrap();
    let opened = crate::workflow::open_verified_file(&path, false).unwrap();
    let budget = crate::workflow::Budget::measured("source bytes", 64, "test").unwrap();

    replace_opened_file(&path, "replacement source");
    let source = crate::workflow::read_opened_utf8_bounded(opened, &budget, 0).unwrap();

    assert_eq!(source, "authorized source");
}

// Covers: replacing an agent file during approval must not change the catalog definition.
// Owner: workflow dynamic capability adapter.
#[cfg(any(unix, windows))]
#[test]
fn agent_parse_uses_the_authorized_open_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("worker.md");
    std::fs::write(
        &path,
        "---\ndescription: authorized\n---\nauthorized prompt\n",
    )
    .unwrap();
    let opened = crate::workflow::open_verified_file(&path, false).unwrap();

    replace_opened_file(
        &path,
        "---\ndescription: replacement\n---\nreplacement prompt\n",
    );
    let source = crate::workflow::read_opened_utf8(opened).unwrap();
    let catalog =
        crate::agent::AgentCatalog::from_authorized_sources(crate::agent::AgentCatalogSources {
            rho_home: vec![(path, source)],
            ..Default::default()
        })
        .unwrap();

    assert_eq!(
        catalog.find("worker").unwrap().definition.description,
        "authorized"
    );
}

// Covers: replacing an authorized catalog root must not change which agent
// files are enumerated or read.
// Owner: workflow dynamic capability adapter.
#[cfg(unix)]
#[test]
fn agent_discovery_stays_on_the_authorized_directory_handle() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("agents");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("worker.md"),
        "---\ndescription: authorized\n---\nauthorized prompt\n",
    )
    .unwrap();
    let opened_root = crate::workflow::open_verified_directory(&root).unwrap();

    std::fs::rename(&root, parent.path().join("authorized-agents")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("worker.md"),
        "---\ndescription: replacement\n---\nreplacement prompt\n",
    )
    .unwrap();

    let names = crate::workflow::opened_directory_names(&opened_root).unwrap();
    assert_eq!(names, vec![std::ffi::OsString::from("worker.md")]);
    let opened = crate::workflow::open_verified_file_in_directory(
        &opened_root,
        Path::new("worker.md"),
        false,
    )
    .unwrap();
    let source = crate::workflow::read_opened_utf8(opened).unwrap();
    let catalog =
        crate::agent::AgentCatalog::from_authorized_sources(crate::agent::AgentCatalogSources {
            rho_home: vec![(root.join("worker.md"), source)],
            ..Default::default()
        })
        .unwrap();

    assert_eq!(
        catalog.find("worker").unwrap().definition.description,
        "authorized"
    );
}

// Covers: executable replacement during approval must freeze the opened executable bytes.
// Owner: workflow dynamic capability adapter.
#[cfg(any(unix, windows))]
#[test]
fn executable_freeze_uses_the_authorized_open_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("command");
    std::fs::write(&path, "authorized executable").unwrap();
    make_executable(&path);
    let opened = crate::workflow::open_executable(&path).unwrap();

    replace_opened_file(&path, "replacement executable");
    make_executable(&path);
    let identity = crate::workflow::freeze_opened_executable(opened, None).unwrap();

    assert_eq!(
        identity.file.content_digest.unwrap().0,
        format!(
            "sha256:{:x}",
            sha2::Sha256::digest(b"authorized executable")
        )
    );
}

// Covers: node resolution must reuse the authorized frozen identity after path replacement.
// Owner: workflow dynamic capability adapter.
#[cfg(any(unix, windows))]
#[test]
fn node_resolution_reuses_the_authorized_executable_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("command");
    std::fs::write(&path, "authorized executable").unwrap();
    make_executable(&path);
    let opened = crate::workflow::open_executable(&path).unwrap();
    let identity = crate::workflow::freeze_opened_executable(opened, None).unwrap();
    let executable = path.to_string_lossy().into_owned();

    replace_opened_file(&path, "replacement executable");
    make_executable(&path);
    let node_id = crate::workflow::test_support::id("command");
    let graph = crate::workflow::WorkflowGraph {
        name: crate::workflow::WorkflowName::new("race").unwrap(),
        nodes: BTreeMap::from([(
            node_id.clone(),
            crate::workflow::Node {
                id: node_id.clone(),
                display_name: "command".into(),
                needs: Vec::new(),
                condition: None,
                execution: crate::workflow::NodeExecution::Command(
                    crate::workflow::CommandNode::Direct {
                        executable: executable.clone(),
                        arguments: Vec::new(),
                        cwd: ".".into(),
                        output: None,
                    },
                ),
                access: crate::workflow::WorkspaceAccess::Mutating,
                allow_failure: false,
                timeout_seconds: 60,
                max_output_bytes: 1024,
            },
        )]),
    };
    let catalog = crate::agent::AgentCatalog::from_authorized_sources(Default::default()).unwrap();
    let available_tools = crate::agent::AgentCapabilities::all_host_tools();
    let config = crate::config::Config::default();
    let executables = BTreeMap::from([(executable, identity.clone())]);
    let host = super::super::AuthorizedPlanHost::new(
        directory.path(),
        &config,
        &catalog,
        &available_tools,
        &executables,
    );
    let resolved = super::super::resolve_nodes_with_host(&graph, &host).unwrap();

    let crate::workflow::ResolvedNode::Command(command) = &resolved[&node_id] else {
        panic!("command node resolved as an agent");
    };
    assert_eq!(command.executable_identity, identity);
}

// Covers: interpreter replacement during approval must freeze the opened interpreter bytes.
// Owner: workflow dynamic capability adapter.
#[cfg(any(unix, windows))]
#[test]
fn interpreter_freeze_uses_the_authorized_open_file() {
    let directory = tempfile::tempdir().unwrap();
    let interpreter_path = directory.path().join("interpreter");
    std::fs::write(&interpreter_path, "authorized interpreter").unwrap();
    make_executable(&interpreter_path);
    let script_path = directory.path().join("script");
    std::fs::write(&script_path, format!("#!{}\n", interpreter_path.display())).unwrap();
    make_executable(&script_path);
    let opened_script = crate::workflow::open_executable(&script_path).unwrap();
    let opened_interpreter = crate::workflow::open_executable(&interpreter_path).unwrap();

    replace_opened_file(&interpreter_path, "replacement interpreter");
    make_executable(&interpreter_path);
    let interpreter = crate::workflow::opened_binary(opened_interpreter).unwrap();
    let identity =
        crate::workflow::freeze_opened_executable(opened_script, Some(interpreter)).unwrap();

    assert_eq!(
        identity.interpreter.unwrap().content_digest.unwrap().0,
        format!(
            "sha256:{:x}",
            sha2::Sha256::digest(b"authorized interpreter")
        )
    );
}
