use std::time::Duration;

use pretty_assertions::assert_eq;

use rho_sdk::{
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget, PathScope, PolicyDecision,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits, WorkspacePolicy,
};

use super::PermissionMode;

fn source(name: &str) -> CapabilitySource {
    CapabilitySource::built_in_tool(name)
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
fn default_mode_is_auto() {
    assert_eq!(PermissionMode::default(), PermissionMode::Auto);
}

#[test]
fn config_value_round_trips_known_modes() {
    for mode in [
        PermissionMode::Auto,
        PermissionMode::Plan,
        PermissionMode::Supervised,
    ] {
        assert_eq!(mode.as_str().parse::<PermissionMode>().unwrap(), mode);
        assert_eq!(
            mode.as_str()
                .to_uppercase()
                .parse::<PermissionMode>()
                .unwrap(),
            mode
        );
    }
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

    let error = "paranoid".parse::<PermissionMode>().unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown permission mode \"paranoid\"; expected auto, plan, or supervised"
    );
    assert!("".parse::<PermissionMode>().is_err());
}

#[test]
fn decision_for_auto_allows_everything() {
    for kind in all_capability_kinds() {
        assert_eq!(
            PermissionMode::Auto.decision_for(kind),
            PolicyDecision::Allow
        );
    }
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
            CapabilityKind::Write | CapabilityKind::Process => PolicyDecision::RequireApproval {
                reason: "host approval is required".into(),
            },
            _ => PolicyDecision::Allow,
        };
        assert_eq!(PermissionMode::Supervised.decision_for(kind), expected);
    }
}

#[test]
fn workspace_policy_agrees_with_decision_for() {
    let read_request = CapabilityRequest::read_path(
        "/workspace/file",
        PathScope::PrimaryWorkspace,
        source("read_file"),
    );
    let write_request = CapabilityRequest::write_path(
        "/workspace/file",
        PathScope::PrimaryWorkspace,
        source("write_file"),
    );
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

    for mode in [PermissionMode::Plan, PermissionMode::Supervised] {
        let policy = mode
            .workspace_policy()
            .expect("policy exists for non-auto modes");
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

    assert!(PermissionMode::Auto.workspace_policy().is_none());
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
