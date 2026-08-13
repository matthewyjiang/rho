use std::process::Command;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use rho_sdk::{
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget, PathScope, PolicyDecision,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits, WorkspacePolicy,
};

use super::{PermissionMode, SessionWriteLog};

fn source(name: &str) -> CapabilitySource {
    CapabilitySource::built_in_tool(name)
}

fn process_request(command: &str) -> CapabilityRequest {
    CapabilityRequest::process(
        ProcessExecution::new(
            "/workspace",
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
            ProcessEnvironment::InheritAll,
            ProcessOutputLimits::new(4096, Some(std::time::Duration::from_secs(30))),
        ),
        source("bash"),
    )
}

fn write_request(path: impl Into<std::path::PathBuf>, scope: PathScope) -> CapabilityRequest {
    CapabilityRequest::write_path(path, scope, source("write"))
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git should be available for permission tests");
    assert!(status.success(), "git {args:?} failed");
}

fn git_workspace_with_tracked_file() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let tracked = dir.path().join("tracked.txt");
    std::fs::write(&tracked, "hello").unwrap();
    run_git(dir.path(), &["init"]);
    run_git(dir.path(), &["add", "tracked.txt"]);
    (dir, tracked)
}

#[test]
fn config_value_parsing_trims_and_rejects_unknown_values() {
    assert_eq!(
        "  Plan  ".parse::<PermissionMode>().unwrap(),
        PermissionMode::Plan
    );
    assert_eq!(
        "SUPERVISED".parse::<PermissionMode>().unwrap(),
        PermissionMode::Supervised
    );
    assert_eq!(
        "allow_edits".parse::<PermissionMode>().unwrap(),
        PermissionMode::AllowEdits
    );
    assert_eq!(
        "allow-edits".parse::<PermissionMode>().unwrap(),
        PermissionMode::AllowEdits
    );
    assert_eq!(PermissionMode::AllowEdits.as_str(), "allow_edits");

    let error = "paranoid".parse::<PermissionMode>().unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown permission mode \"paranoid\"; expected bypass, auto, allow_edits, plan, or supervised"
    );
    assert!("".parse::<PermissionMode>().is_err());
}

#[test]
fn decision_for_bypass_allows_everything() {
    for kind in all_capability_kinds() {
        assert_eq!(
            PermissionMode::Bypass.decision_for(kind),
            PolicyDecision::Allow
        );
    }
}

#[test]
fn decision_for_auto_matches_allow_edits_and_supervised() {
    for kind in all_capability_kinds() {
        assert_eq!(
            PermissionMode::Auto.decision_for(kind),
            PermissionMode::AllowEdits.decision_for(kind)
        );
        assert_eq!(
            PermissionMode::AllowEdits.decision_for(kind),
            PermissionMode::Supervised.decision_for(kind)
        );
    }
}

#[test]
fn parse_auto_is_classifier_mode_and_bypass_is_no_checks() {
    assert_eq!(
        "auto".parse::<PermissionMode>().unwrap(),
        PermissionMode::Auto
    );
    assert_eq!(
        "bypass".parse::<PermissionMode>().unwrap(),
        PermissionMode::Bypass
    );
    assert_eq!(PermissionMode::default(), PermissionMode::Bypass);
}

#[test]
fn decision_for_plan_denies_only_write_and_process() {
    for kind in all_capability_kinds() {
        let expected = match kind {
            CapabilityKind::Write | CapabilityKind::Process => PolicyDecision::Deny {
                reason: "capability is not allowed in plan mode".into(),
            },
            _ => PolicyDecision::Allow,
        };
        assert_eq!(PermissionMode::Plan.decision_for(kind), expected);
    }
}

#[test]
fn decision_for_supervised_requires_approval_only_for_write_and_process() {
    for kind in all_capability_kinds() {
        let expected = match kind {
            // Boilerplate reasons stay empty; the approval surface is the signal.
            CapabilityKind::Write | CapabilityKind::Process => PolicyDecision::RequireApproval {
                reason: String::new(),
            },
            _ => PolicyDecision::Allow,
        };
        assert_eq!(PermissionMode::Supervised.decision_for(kind), expected);
    }
}

#[test]
fn workspace_policy_agrees_with_decision_for_when_writes_are_not_tracked() {
    let read_request = CapabilityRequest::read_path(
        "/workspace/file",
        PathScope::PrimaryWorkspace,
        source("read_file"),
    );
    let write_request = write_request("/workspace/file", PathScope::PrimaryWorkspace);
    let network_request = CapabilityRequest::network(
        NetworkTarget::Url("https://example.com/path".into()),
        source("fetch_content"),
    );
    let skill_request = CapabilityRequest::skill("test", None, source("skill"));
    let instruction_request = CapabilityRequest::instruction_discovery(
        "/workspace/AGENTS.md",
        PathScope::PrimaryWorkspace,
        CapabilitySource::PromptConstruction,
    );
    let process = process_request("cargo test");

    for mode in [
        PermissionMode::Auto,
        PermissionMode::AllowEdits,
        PermissionMode::Plan,
        PermissionMode::Supervised,
    ] {
        let policy = mode
            .workspace_policy(SessionWriteLog::default())
            .expect("policy exists for checked modes");
        for request in [
            &read_request,
            &write_request,
            &network_request,
            &skill_request,
            &instruction_request,
            &process,
        ] {
            assert_eq!(
                policy.evaluate(request),
                mode.decision_for(request.kind()),
                "mode {:?} disagreed for kind {:?}",
                mode,
                request.kind()
            );
        }
    }

    assert!(PermissionMode::Bypass
        .workspace_policy(SessionWriteLog::default())
        .is_none());
}

// Covers: Allow edits and Auto skip the classifier/prompt for in-workspace
// writes to git-tracked files, but not for new files, out-of-workspace paths,
// or process execution.
// Owner: application permission policy
#[test]
fn allow_edits_and_auto_allow_tracked_workspace_writes_only() {
    let (_dir, tracked) = git_workspace_with_tracked_file();
    let untracked = tracked.with_file_name("untracked.txt");
    std::fs::write(&untracked, "new").unwrap();

    let tracked_write = write_request(tracked.clone(), PathScope::PrimaryWorkspace);
    let untracked_write = write_request(untracked, PathScope::PrimaryWorkspace);
    let outside_write = write_request(tracked, PathScope::UnrestrictedFilesystem);
    let process = process_request("git status");

    for mode in [PermissionMode::AllowEdits, PermissionMode::Auto] {
        let policy = mode
            .workspace_policy(SessionWriteLog::default())
            .expect("checked mode has a policy");
        assert_eq!(policy.evaluate(&tracked_write), PolicyDecision::Allow);
        assert_eq!(
            policy.evaluate(&untracked_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
        assert_eq!(
            policy.evaluate(&outside_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
        assert_eq!(
            policy.evaluate(&process),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
    }

    let supervised = PermissionMode::Supervised
        .workspace_policy(SessionWriteLog::default())
        .expect("supervised has a policy");
    assert_eq!(
        supervised.evaluate(&tracked_write),
        PolicyDecision::RequireApproval {
            reason: String::new(),
        }
    );
}

// Covers: after a new workspace file is allowed once, later edits skip the
// gate; ignored and out-of-workspace paths stay gated.
// Owner: application permission policy
#[test]
fn allow_edits_and_auto_allow_later_writes_to_approved_workspace_files() {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init"]);
    std::fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
    let created = dir.path().join("new.rs");
    let other = dir.path().join("other.rs");
    let ignored = dir.path().join(".env");
    std::fs::write(&created, "fn main() {}\n").unwrap();
    std::fs::write(&other, "fn other() {}\n").unwrap();
    std::fs::write(&ignored, "SECRET=\n").unwrap();

    let created_write = write_request(created, PathScope::PrimaryWorkspace);
    let other_write = write_request(other, PathScope::PrimaryWorkspace);
    let ignored_write = write_request(ignored, PathScope::PrimaryWorkspace);
    let outside_write = write_request(dir.path().join("new.rs"), PathScope::UnrestrictedFilesystem);

    for mode in [PermissionMode::AllowEdits, PermissionMode::Auto] {
        let policy = mode
            .workspace_policy(SessionWriteLog::default())
            .expect("checked mode has a policy");
        assert_eq!(
            policy.evaluate(&created_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
        policy.remember_approved_write(&created_write);
        policy.remember_approved_write(&ignored_write);
        policy.remember_approved_write(&outside_write);
        assert_eq!(policy.evaluate(&created_write), PolicyDecision::Allow);
        assert_eq!(
            policy.evaluate(&other_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
        assert_eq!(
            policy.evaluate(&ignored_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
        assert_eq!(
            policy.evaluate(&outside_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
    }
}

// Covers: a symlink never gets the free-write skip, even when git tracks it or
// the session already allowed that path, so a link out of the workspace is
// still gated.
// Owner: application permission policy
#[cfg(unix)]
#[test]
fn allow_edits_and_auto_gate_symlinked_workspace_writes() {
    let outside = TempDir::new().unwrap();
    let target = outside.path().join("secret.txt");
    std::fs::write(&target, "secret").unwrap();

    let dir = TempDir::new().unwrap();
    let tracked_link = dir.path().join("tracked-link.txt");
    std::os::unix::fs::symlink(&target, &tracked_link).unwrap();
    run_git(dir.path(), &["init"]);
    run_git(dir.path(), &["add", "tracked-link.txt"]);

    // A path allowed earlier this session, then swapped for a link.
    let remembered = dir.path().join("remembered.txt");
    std::fs::write(&remembered, "plain").unwrap();

    let tracked_link_write = write_request(tracked_link, PathScope::PrimaryWorkspace);
    let remembered_write = write_request(remembered.clone(), PathScope::PrimaryWorkspace);

    for mode in [PermissionMode::AllowEdits, PermissionMode::Auto] {
        let policy = mode
            .workspace_policy(SessionWriteLog::default())
            .expect("checked mode has a policy");
        assert_eq!(
            policy.evaluate(&tracked_link_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );

        policy.remember_approved_write(&remembered_write);
        assert_eq!(policy.evaluate(&remembered_write), PolicyDecision::Allow);
        std::fs::remove_file(&remembered).unwrap();
        std::os::unix::fs::symlink(&target, &remembered).unwrap();
        assert_eq!(
            policy.evaluate(&remembered_write),
            PolicyDecision::RequireApproval {
                reason: String::new(),
            }
        );
        std::fs::remove_file(&remembered).unwrap();
        std::fs::write(&remembered, "plain").unwrap();
    }
}

fn all_capability_kinds() -> [CapabilityKind; 6] {
    [
        CapabilityKind::Read,
        CapabilityKind::Write,
        CapabilityKind::Process,
        CapabilityKind::Network,
        CapabilityKind::Skill,
        CapabilityKind::InstructionDiscovery,
    ]
}
