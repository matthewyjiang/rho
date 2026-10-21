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

// Covers: a direct executable reports no shell command, and environment
// variable names never reach a hook payload.
// Owner: SDK hook payload construction.
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
        serde_json::to_value(&capability).unwrap(),
        json!({
            "operation": "execute_process",
            "working_directory": "/work",
            "executable": "/usr/bin/env",
            "arguments": [],
            "shell_command": null,
            "environment": "inherit_listed",
        })
    );
    assert!(
        !encoded.contains("ANTHROPIC_API_KEY"),
        "variable names leaked into the payload: {encoded}"
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

// Covers: network summaries drop userinfo and query strings, and a
// tool-managed destination reports no URL.
// Owner: SDK hook payload construction.
#[test]
fn a_network_summary_drops_credentials_and_query_strings() {
    let cases = [
        (
            NetworkTarget::Url(
                "https://user:secret@api.example.com/v1/data?token=abcd#frag".into(),
            ),
            json!({
                "operation": "network_access",
                "url": "https://api.example.com/v1/data",
                "host": "api.example.com",
                "query_present": true,
            }),
        ),
        (
            NetworkTarget::ToolManaged,
            json!({
                "operation": "network_access",
                "url": null,
                "host": null,
                "query_present": false,
            }),
        ),
    ];

    for (target, expected) in cases {
        let request =
            CapabilityRequest::network(target, CapabilitySource::built_in_tool("fetch_content"));
        let (capability, _) = summarize(&request);
        let encoded = serde_json::to_string(&capability).unwrap();

        assert_eq!(serde_json::to_value(&capability).unwrap(), expected);
        assert!(!encoded.contains("secret"), "credentials leaked: {encoded}");
        assert!(!encoded.contains("abcd"), "query leaked: {encoded}");
    }
}

// Covers: every host-controlled capability scalar must honor the field budget.
// Owner: SDK hook payload construction.
#[test]
fn externally_supplied_capability_fields_are_bounded() {
    let long = "x".repeat(64);
    let cases = [
        (shell(&long), vec!["payload.capability.shell_command"]),
        (
            CapabilityRequest::process(
                ProcessExecution::new(
                    "/work",
                    ProcessInvocation::executable("/bin/echo", vec!["ok".into(), long.clone()]),
                    ProcessEnvironment::Empty,
                    ProcessOutputLimits::new(1024, None),
                ),
                CapabilitySource::built_in_tool("process"),
            ),
            vec![
                "payload.capability.arguments[1]",
                "payload.capability.executable",
            ],
        ),
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
fn matchers_read_tool_name_and_status_from_the_payload() {
    let after = HookPayload::AfterToolUse(AfterToolUsePayload {
        tool: tool("bash", Some("c1")),
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
