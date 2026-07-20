use rho_sdk::{tool::OperationKind, CapabilityKind};

use crate::tool::{Tool, ToolContext, ToolError, ToolResult, ToolSpec};

struct NamedLegacyTool(&'static str);

#[async_trait::async_trait]
impl Tool for NamedLegacyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.0.into(),
            description: "test legacy tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        _context: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            id,
            ok: true,
            content: "ok".into(),
        })
    }
}

#[test]
fn process_profile_carries_security_and_metadata_into_sdk_contract() {
    let tool = super::process(NamedLegacyTool("process"), 1_000).expect("matching process tool");

    assert_eq!(tool.spec().name, "process");
    assert_eq!(tool.security().capabilities(), [CapabilityKind::Process]);
    assert_eq!(
        tool.start_metadata(&serde_json::json!({})).operation_kind(),
        Some(&OperationKind::Execute)
    );
}

#[test]
fn web_search_profile_declares_network_access() {
    let tool =
        super::web_search(NamedLegacyTool("web_search"), 1_000).expect("matching web search tool");

    assert_eq!(tool.security().capabilities(), [CapabilityKind::Network]);
    assert_eq!(
        tool.start_metadata(&serde_json::json!({})).operation_kind(),
        Some(&OperationKind::Network)
    );
}

#[test]
fn rejects_a_tool_that_mismatches_the_trusted_profile() {
    let error = match super::agent(NamedLegacyTool("process"), 1_000) {
        Ok(_) => panic!("mismatched legacy tool was adapted"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "legacy tool profile 'agent' does not match wrapped tool 'process'"
    );
}

#[test]
fn rejects_an_unknown_tool_instead_of_granting_builtin_trust() {
    let error = match super::rho(NamedLegacyTool("unknown_legacy"), 1_000) {
        Ok(_) => panic!("unknown legacy tool was adapted"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "legacy tool profile 'rho' does not match wrapped tool 'unknown_legacy'"
    );
}
