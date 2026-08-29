use pretty_assertions::assert_eq;
use serde_json::json;

use crate::workspace::{
    CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget, PathScope,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};

use super::*;

fn bounds() -> HookPayloadBounds {
    HookPayloadBounds::default()
}

fn summarize(request: &CapabilityRequest) -> (HookCapability, HookTruncation) {
    let mut truncation = HookTruncation::default();
    let capability = summarize_capability(request, bounds(), &mut truncation);
    (capability, truncation)
}

fn tool(name: &str, call_id: Option<&str>) -> HookTool {
    HookTool::new(
        name,
        call_id.map(str::to_owned),
        bounds(),
        &mut HookTruncation::default(),
    )
}

fn shell(command: &str) -> CapabilityRequest {
    CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
            ProcessEnvironment::InheritAll,
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool("bash"),
    )
}

#[test]
fn a_read_summary_keeps_the_path_and_scope() {
    let request = CapabilityRequest::read_path(
        "/work/src/lib.rs",
        PathScope::PrimaryWorkspace,
        CapabilitySource::built_in_tool("read_file"),
    );

    let (capability, truncation) = summarize(&request);

    assert_eq!(
        serde_json::to_value(&capability).unwrap(),
        json!({
            "operation": "read_path",
            "path": "/work/src/lib.rs",
            "scope": "primary_workspace",
        })
    );
    assert!(!truncation.is_truncated());
}

#[test]
fn a_shell_summary_exposes_the_command_a_deny_hook_must_inspect() {
    let (capability, _) = summarize(&shell("git push --force"));

    assert_eq!(
        serde_json::to_value(&capability).unwrap(),
        json!({
            "operation": "execute_process",
            "working_directory": "/work",
            "executable": "bash",
            "arguments": ["-lc"],
            "shell_command": "git push --force",
            "environment": "inherit_all",
        })
    );
}

#[test]
fn a_direct_executable_summary_reports_no_shell_command() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::executable("/usr/bin/git", vec!["status".into()]),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool("process"),
    );

    let (capability, _) = summarize(&request);

    assert_eq!(
        serde_json::to_value(&capability).unwrap()["shell_command"],
        serde_json::Value::Null
    );
}

#[test]
fn environment_values_never_reach_a_payload() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::executable("/usr/bin/env", Vec::new()),
            ProcessEnvironment::InheritListed {
                variable_names: vec!["ANTHROPIC_API_KEY".into()],
            },
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool("process"),
    );

    let (capability, _) = summarize(&request);
    let encoded = serde_json::to_string(&capability).unwrap();

    assert_eq!(
        serde_json::to_value(&capability).unwrap()["environment"],
        json!("inherit_listed")
    );
    assert!(
        !encoded.contains("ANTHROPIC_API_KEY"),
        "variable names leaked into the payload: {encoded}"
    );
}

#[test]
fn a_long_shell_command_is_cut_and_named_in_the_report() {
    let mut truncation = HookTruncation::default();
    let request = shell(&"x".repeat(64));

    let capability = summarize_capability(
        &request,
        HookPayloadBounds::new(
            /* max_field_bytes */ 8, /* max_envelope_bytes */ 4096,
        ),
        &mut truncation,
    );

    assert_eq!(
        serde_json::to_value(&capability).unwrap()["shell_command"],
        json!("xxxxxxxx")
    );
    assert_eq!(
        truncation.fields().collect::<Vec<_>>(),
        vec!["payload.capability.shell_command"]
    );
}

#[test]
fn a_long_argument_is_cut_and_named_by_index() {
    let mut truncation = HookTruncation::default();
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::executable("/bin/echo", vec!["ok".into(), "y".repeat(64)]),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool("process"),
    );

    summarize_capability(
        &request,
        HookPayloadBounds::new(
            /* max_field_bytes */ 8, /* max_envelope_bytes */ 4096,
        ),
        &mut truncation,
    );

    assert_eq!(
        truncation.fields().collect::<Vec<_>>(),
        vec![
            "payload.capability.arguments[1]",
            "payload.capability.executable",
        ]
    );
}

// Covers: wide process argument lists must degrade to a truncated hook payload.
// Owner: SDK hook payload construction.
#[test]
fn total_arguments_are_bounded_and_reported() {
    let mut truncation = HookTruncation::default();
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::executable("/bin/echo", vec!["1234".into(); 8]),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool("process"),
    );

    let capability = summarize_capability(
        &request,
        HookPayloadBounds::new(
            /* max_field_bytes */ 8, /* max_envelope_bytes */ 12,
        ),
        &mut truncation,
    );

    let HookCapability::ExecuteProcess { arguments, .. } = capability else {
        panic!("expected process capability")
    };
    assert_eq!(arguments, vec!["1234", "1234", "1234"]);
    assert_eq!(
        truncation.fields().collect::<Vec<_>>(),
        vec![
            "payload.capability.arguments",
            "payload.capability.executable",
        ]
    );
}

#[test]
fn a_network_summary_drops_credentials_and_query_strings() {
    let request = CapabilityRequest::network(
        NetworkTarget::Url("https://user:secret@api.example.com/v1/data?token=abcd#frag".into()),
        CapabilitySource::built_in_tool("fetch_content"),
    );

    let (capability, _) = summarize(&request);
    let encoded = serde_json::to_string(&capability).unwrap();

    assert_eq!(
        serde_json::to_value(&capability).unwrap(),
        json!({
            "operation": "network_access",
            "url": "https://api.example.com/v1/data",
            "host": "api.example.com",
            "query_present": true,
        })
    );
    assert!(!encoded.contains("secret"), "credentials leaked: {encoded}");
    assert!(!encoded.contains("abcd"), "query leaked: {encoded}");
}

#[test]
fn a_tool_managed_destination_reports_no_url() {
    let request = CapabilityRequest::network(
        NetworkTarget::ToolManaged,
        CapabilitySource::built_in_tool("web_search"),
    );

    let (capability, _) = summarize(&request);

    assert_eq!(
        serde_json::to_value(&capability).unwrap(),
        json!({
            "operation": "network_access",
            "url": null,
            "host": null,
            "query_present": false,
        })
    );
}

// Covers: every host-controlled capability scalar must honor the field budget.
// Owner: SDK hook payload construction.
#[test]
fn externally_supplied_capability_fields_are_bounded() {
    let long = "x".repeat(64);
    let cases = [
        (
            CapabilityRequest::read_path(
                &long,
                PathScope::PrimaryWorkspace,
                CapabilitySource::built_in_tool("read_file"),
            ),
            vec!["payload.capability.path"],
        ),
        (
            CapabilityRequest::network(
                NetworkTarget::Url(format!("https://example.com/{long}")),
                CapabilitySource::built_in_tool("fetch_content"),
            ),
            vec!["payload.capability.host", "payload.capability.url"],
        ),
        (
            CapabilityRequest::new(
                CapabilityOperation::LoadSkill {
                    name: long.clone(),
                    path: Some(long.clone().into()),
                },
                CapabilitySource::built_in_tool("skill"),
            ),
            vec!["payload.capability.name", "payload.capability.path"],
        ),
    ];

    for (request, expected_fields) in cases {
        let mut truncation = HookTruncation::default();
        let capability =
            summarize_capability(&request, HookPayloadBounds::new(8, 4096), &mut truncation);

        assert!(serde_json::to_vec(&capability).unwrap().len() < 4096);
        assert_eq!(truncation.fields().collect::<Vec<_>>(), expected_fields);
    }
}

#[test]
fn tool_identity_fields_are_bounded() {
    let mut truncation = HookTruncation::default();
    let tool = HookTool::new(
        "tool-name-is-long",
        Some("call-id-is-long".into()),
        HookPayloadBounds::new(8, 4096),
        &mut truncation,
    );

    assert_eq!(
        (tool.name, tool.call_id),
        ("tool-nam".into(), Some("call-id-".into()))
    );
    assert_eq!(
        truncation.fields().collect::<Vec<_>>(),
        vec!["payload.tool.call_id", "payload.tool.name"]
    );
}

#[test]
fn failure_kind_and_message_are_bounded() {
    let mut truncation = HookTruncation::default();
    let failure = bounded_failure(
        BoundedFailure {
            kind: "failure-kind-is-long",
            message: "failure-message-is-long",
            field: "payload.failure",
        },
        HookPayloadBounds::new(8, 4096),
        &mut truncation,
    );

    assert_eq!(
        failure,
        HookFailure {
            kind: "failure-".into(),
            message: "failure-".into(),
        }
    );
    assert_eq!(
        truncation.fields().collect::<Vec<_>>(),
        vec!["payload.failure.kind", "payload.failure.message"]
    );
}

#[test]
fn tool_identity_comes_from_the_capability_source() {
    let mut truncation = HookTruncation::default();
    assert_eq!(
        HookTool::from_source(
            &CapabilitySource::built_in_tool("bash"),
            Some("c1".into()),
            bounds(),
            &mut truncation,
        ),
        HookTool {
            name: "bash".into(),
            call_id: Some("c1".into()),
        }
    );
    assert_eq!(
        HookTool::from_source(
            &CapabilitySource::host_tool("deploy"),
            None,
            bounds(),
            &mut truncation,
        ),
        HookTool {
            name: "deploy".into(),
            call_id: None,
        }
    );
    assert_eq!(
        HookTool::from_source(
            &CapabilitySource::PromptConstruction,
            None,
            bounds(),
            &mut truncation,
        )
        .name,
        PROMPT_CONSTRUCTION_TOOL
    );
}

#[test]
fn matchers_read_tool_name_and_status_from_the_payload() {
    let after = HookPayload::AfterToolUse(AfterToolUsePayload {
        tool: tool("bash", Some("c1")),
        capability: None,
        status: HookToolStatus::Failed,
        failure: None,
        duration_ms: Some(12),
    });

    assert_eq!(after.tool_name(), Some("bash"));
    assert_eq!(after.succeeded(), Some(false));

    let started = HookPayload::SessionStarted(SessionStartedPayload {
        model: "anthropic/opus".into(),
    });
    assert_eq!(started.tool_name(), None);
    assert_eq!(started.succeeded(), None);
}

#[test]
fn every_sdk_error_has_a_stable_label() {
    let labelled = [
        (crate::Error::Cancelled, "cancelled"),
        (crate::Error::RuntimeShutdown, "runtime_shutdown"),
        (crate::Error::SessionBusy, "session_busy"),
        (
            crate::Error::PolicyDenied {
                message: "no".into(),
            },
            "policy_denied",
        ),
    ];
    for (error, expected) in labelled {
        assert_eq!(error_label(&error), expected);
    }
}

#[test]
fn a_policy_denial_produces_no_hook_outcome() {
    assert_eq!(
        HookPolicyOutcome::from_policy(&crate::PolicyDecision::Allow),
        Some(HookPolicyOutcome::Allow)
    );
    assert_eq!(
        HookPolicyOutcome::from_policy(&crate::PolicyDecision::RequireApproval {
            reason: "ask".into()
        }),
        Some(HookPolicyOutcome::RequireApproval)
    );
    assert_eq!(
        HookPolicyOutcome::from_policy(&crate::PolicyDecision::Deny {
            reason: "no".into()
        }),
        None
    );
}
