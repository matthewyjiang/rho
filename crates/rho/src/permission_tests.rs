use std::process::Command;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use rho_sdk::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest, CapabilityKind,
    CapabilityRequest, CapabilitySource, NetworkTarget, PathScope, PolicyDecision,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits, WorkspacePolicy,
};

use super::{remember_allowed_workspace_writes, PermissionMode, SessionWriteLog, WriteAuthority};

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

fn read_request(path: impl Into<std::path::PathBuf>, scope: PathScope) -> CapabilityRequest {
    CapabilityRequest::read_path(path, scope, source("read_file"))
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
    let read_request = read_request("/workspace/file", PathScope::PrimaryWorkspace);
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

// Covers: checked modes no longer silently allow reads of arbitrary filesystem
// paths; primary-workspace and host-attached roots stay free. There is no
// secret-path denylist — PathScope is the policy input.
// Owner: application permission policy
#[test]
fn checked_modes_gate_unrestricted_filesystem_reads() {
    let require_approval = PolicyDecision::RequireApproval {
        reason: String::new(),
    };
    let plan_deny = PolicyDecision::Deny {
        reason: "read outside the workspace is not allowed in plan mode".into(),
    };
    let cases = [
        (
            PathScope::PrimaryWorkspace,
            PolicyDecision::Allow,
            PolicyDecision::Allow,
        ),
        (
            PathScope::GrantedRoot {
                root: "/extra".into(),
            },
            PolicyDecision::Allow,
            PolicyDecision::Allow,
        ),
        (
            PathScope::UnrestrictedFilesystem,
            require_approval,
            plan_deny,
        ),
    ];

    for (scope, gated, plan) in cases {
        let request = read_request("/tmp/file", scope.clone());
        for mode in [
            PermissionMode::Auto,
            PermissionMode::AllowEdits,
            PermissionMode::Supervised,
        ] {
            let policy = mode
                .workspace_policy(SessionWriteLog::default())
                .expect("checked mode has a policy");
            assert_eq!(
                policy.evaluate(&request),
                gated.clone(),
                "{mode:?} disagreed for {scope:?}"
            );
        }
        let plan_policy = PermissionMode::Plan
            .workspace_policy(SessionWriteLog::default())
            .expect("plan has a policy");
        assert_eq!(
            plan_policy.evaluate(&request),
            plan,
            "plan disagreed for {scope:?}"
        );
    }
}

// Covers: global AGENTS.md and user skill trees stay readable and editable
// without opening the rest of ~/.rho (credentials, config).
// Owner: application permission policy
#[test]
fn user_instruction_and_skill_paths_are_workspace_scoped() {
    let home = TempDir::new().unwrap();
    let home = home.path();
    let agents = crate::paths::user_agents_md(home);
    let [rho_skills, agents_skills] = crate::paths::user_skill_dirs(home);
    let skill = rho_skills.join("demo").join("SKILL.md");
    let other_skill = agents_skills.join("demo").join("SKILL.md");
    let credentials = home.join(".rho").join("credentials").join("secrets.json");
    let config = home.join(".rho").join("config.toml");

    let allow = PolicyDecision::Allow;
    let require_approval = PolicyDecision::RequireApproval {
        reason: String::new(),
    };
    let plan_write_deny = PolicyDecision::Deny {
        reason: "capability is not allowed in plan mode".into(),
    };
    let plan_read_deny = PolicyDecision::Deny {
        reason: "read outside the workspace is not allowed in plan mode".into(),
    };

    let cases = [
        (
            read_request(&agents, PathScope::UnrestrictedFilesystem),
            allow.clone(),
            allow.clone(),
            allow.clone(),
        ),
        (
            read_request(&skill, PathScope::UnrestrictedFilesystem),
            allow.clone(),
            allow.clone(),
            allow.clone(),
        ),
        (
            read_request(&other_skill, PathScope::UnrestrictedFilesystem),
            allow.clone(),
            allow.clone(),
            allow.clone(),
        ),
        (
            write_request(&agents, PathScope::UnrestrictedFilesystem),
            allow.clone(),
            require_approval.clone(),
            plan_write_deny.clone(),
        ),
        (
            write_request(&skill, PathScope::UnrestrictedFilesystem),
            allow.clone(),
            require_approval.clone(),
            plan_write_deny.clone(),
        ),
        (
            read_request(&credentials, PathScope::UnrestrictedFilesystem),
            require_approval.clone(),
            require_approval.clone(),
            plan_read_deny.clone(),
        ),
        (
            read_request(&config, PathScope::UnrestrictedFilesystem),
            require_approval.clone(),
            require_approval.clone(),
            plan_read_deny,
        ),
        (
            write_request(&credentials, PathScope::UnrestrictedFilesystem),
            require_approval.clone(),
            require_approval,
            plan_write_deny,
        ),
    ];

    for (request, auto, supervised, plan) in cases {
        for mode in [PermissionMode::Auto, PermissionMode::AllowEdits] {
            let policy = super::ModePolicy::for_home(mode, home);
            assert_eq!(
                policy.evaluate(&request),
                auto.clone(),
                "{mode:?} disagreed for {:?}",
                request.operation()
            );
        }
        assert_eq!(
            super::ModePolicy::for_home(PermissionMode::Supervised, home).evaluate(&request),
            supervised,
            "supervised disagreed for {:?}",
            request.operation()
        );
        assert_eq!(
            super::ModePolicy::for_home(PermissionMode::Plan, home).evaluate(&request),
            plan,
            "plan disagreed for {:?}",
            request.operation()
        );
    }
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

// Covers: classifier approval on a shared log cannot skip Allow edits' human gate.
// Owner: application permission policy
#[tokio::test]
async fn remembered_writes_are_bound_to_their_grantor() {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init"]);
    let created = dir.path().join("new.rs");
    std::fs::write(&created, "fn main() {}\n").unwrap();
    let created_write = write_request(created, PathScope::PrimaryWorkspace);

    struct Allow;
    impl ApprovalHandler for Allow {
        fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
            Box::pin(async { ApprovalDecision::AllowOnce })
        }
    }

    let writes = SessionWriteLog::default();
    let handler = remember_allowed_workspace_writes(
        std::sync::Arc::new(Allow),
        writes.clone(),
        WriteAuthority::Classifier,
    );
    handler
        .request(ApprovalRequest::new(created_write.clone(), ""))
        .await;

    let auto = PermissionMode::Auto
        .workspace_policy(writes.clone())
        .expect("auto has a policy");
    let allow_edits = PermissionMode::AllowEdits
        .workspace_policy(writes)
        .expect("allow edits has a policy");
    assert_eq!(auto.evaluate(&created_write), PolicyDecision::Allow);
    assert_eq!(
        allow_edits.evaluate(&created_write),
        PolicyDecision::RequireApproval {
            reason: String::new(),
        }
    );
}

#[test]
fn carried_across_keeps_writes_only_for_the_same_grantor() {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init"]);
    let created = dir.path().join("new.rs");
    std::fs::write(&created, "fn main() {}\n").unwrap();
    let created_write = write_request(created, PathScope::PrimaryWorkspace);

    let writes = SessionWriteLog::default();
    writes.remember(&created_write, WriteAuthority::Classifier);
    let carried = writes
        .clone()
        .carried_across(PermissionMode::Auto, PermissionMode::AllowEdits);
    let allow_edits = PermissionMode::AllowEdits
        .workspace_policy(carried)
        .expect("allow edits has a policy");
    assert_eq!(
        allow_edits.evaluate(&created_write),
        PolicyDecision::RequireApproval {
            reason: String::new(),
        }
    );

    let same_session = writes.carried_across(PermissionMode::Auto, PermissionMode::Auto);
    let auto = PermissionMode::Auto
        .workspace_policy(same_session)
        .expect("auto has a policy");
    assert_eq!(auto.evaluate(&created_write), PolicyDecision::Allow);
}

// Covers: a human grant is accepted in Auto, but a classifier grant is never
// accepted in Allow edits.
// Owner: application permission policy
#[test]
fn human_grants_are_stronger_than_classifier_grants() {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init"]);
    let created = dir.path().join("new.rs");
    std::fs::write(&created, "fn main() {}\n").unwrap();
    let created_write = write_request(created, PathScope::PrimaryWorkspace);

    let writes = SessionWriteLog::default();
    writes.remember(&created_write, WriteAuthority::Human);
    for mode in [PermissionMode::Auto, PermissionMode::AllowEdits] {
        let policy = mode
            .workspace_policy(writes.clone())
            .expect("edit-allowing mode has a policy");
        assert_eq!(
            policy.evaluate(&created_write),
            PolicyDecision::Allow,
            "{mode:?} should honor a human grant"
        );
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
