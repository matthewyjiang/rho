use std::{num::NonZeroUsize, str::FromStr};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{model::ToolSpec, CancellationToken, ToolCallId};

use super::{
    tool_progress_channel, ScriptedTool, ScriptedToolOutcome, Tool, ToolContext, ToolErrorKind,
    ToolInvocation, ToolOutput, ToolRegistry, ToolResource,
};

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("{name} description"),
        input_schema: json!({"type": "object"}),
    }
}

fn invocation() -> ToolInvocation {
    ToolInvocation::new(ToolCallId::from_str("call-1").unwrap(), json!({"value": 1}))
}

fn context(cancellation: CancellationToken) -> (ToolContext, super::ToolProgressReceiver) {
    let (progress, receiver) = tool_progress_channel(NonZeroUsize::new(4).unwrap());
    (
        ToolContext::new(
            Some(crate::Workspace::new(std::env::temp_dir()).unwrap()),
            cancellation,
            progress,
        ),
        receiver,
    )
}

#[test]
fn resource_debug_output_omits_secret_values() {
    let resource = ToolResource::opaque("private-tool", "secret-key");
    let debug = format!("{resource:?}");

    assert!(debug.contains("Opaque"));
    assert!(!debug.contains("private-tool"));
    assert!(!debug.contains("secret-key"));
}

#[test]
fn registry_rejects_duplicate_names_without_replacing_the_first_tool() {
    let first = ScriptedTool::new(
        spec("duplicate"),
        ScriptedToolOutcome::Success(ToolOutput::text("first")),
    );
    let second = ScriptedTool::new(
        spec("duplicate"),
        ScriptedToolOutcome::Success(ToolOutput::text("second")),
    );
    let mut registry = ToolRegistry::new();
    registry.register(first).unwrap();

    let error = registry.register(second).unwrap_err();

    assert_eq!(error.name(), "duplicate");
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>(),
        ["duplicate"]
    );
}

/// Tool that overrides neither `call` nor `prepare`, which is a programming
/// error the SDK must report rather than loop on.
struct UnimplementedTool;

impl Tool for UnimplementedTool {
    fn spec(&self) -> ToolSpec {
        spec("unimplemented")
    }
}

#[tokio::test]
async fn a_tool_implementing_neither_call_nor_prepare_reports_the_mistake() {
    let (context, _receiver) = context(CancellationToken::new());

    let error = UnimplementedTool
        .call(invocation(), context)
        .await
        .expect_err("a tool with no implementation cannot succeed");

    assert_eq!(error.kind(), ToolErrorKind::Execution);
    assert!(
        error
            .message()
            .contains("implements neither Tool::call nor Tool::prepare"),
        "unexpected message: {}",
        error.message()
    );
}
