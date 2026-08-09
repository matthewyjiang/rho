use pretty_assertions::assert_eq;

use super::{client_info, McpClientServices, McpElicitationService, McpRoots, McpSamplingService};
use crate::tools::mcp::{
    config::McpSamplingPolicy, elicitation::McpElicitationSupport, inflight::McpInFlightCalls,
    sampling::McpSamplingBridge,
};

fn services(
    elicitation: McpElicitationSupport,
    sampling: Option<McpSamplingPolicy>,
) -> McpClientServices {
    let calls = McpInFlightCalls::new();
    McpClientServices {
        elicit: McpElicitationService::new("live", calls.clone(), elicitation),
        sample: sampling
            .map(|policy| McpSamplingService::new("live", policy, McpSamplingBridge::new(), calls)),
    }
}

// Covers: `initialize` must identify Rho rather than the client library, must
// declare roots only when there is a workspace to serve, and must not promise a
// `roots/list_changed` notification Rho never sends.
// Owner: MCP client identity and capability declaration.
#[test]
fn client_info_identifies_rho_and_declares_roots_when_present() {
    let directory = tempfile::tempdir().unwrap();
    let with_roots = client_info(
        &McpRoots::for_workspace(directory.path()),
        &services(McpElicitationSupport::Unavailable, None),
    );

    assert_eq!(
        (
            with_roots.client_info.name.as_str(),
            with_roots.client_info.version.as_str(),
        ),
        ("rho", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        with_roots
            .capabilities
            .roots
            .and_then(|roots| roots.list_changed),
        Some(false)
    );

    let without_roots = client_info(
        &McpRoots::default(),
        &services(McpElicitationSupport::Unavailable, None),
    );
    assert!(without_roots.capabilities.roots.is_none());
}

// Covers: Rho must declare elicitation and sampling only where it can serve
// them, because a server reads the capability set as a promise and a declared
// capability Rho always refuses is worse than one it never offered.
// Owner: MCP client capability declaration.
#[test]
fn optional_capabilities_are_declared_only_when_serviceable() {
    let none = client_info(
        &McpRoots::default(),
        &services(McpElicitationSupport::Unavailable, None),
    );
    assert!(none.capabilities.elicitation.is_none());
    assert!(none.capabilities.sampling.is_none());

    let both = client_info(
        &McpRoots::default(),
        &services(
            McpElicitationSupport::Available,
            Some(McpSamplingPolicy::Ask),
        ),
    );
    // Form mode only, and schema validation is declared off because Rho types
    // answers to the schema without enforcing its constraints.
    let elicitation = both
        .capabilities
        .elicitation
        .expect("elicitation is declared when a run can ask a person");
    assert_eq!(
        elicitation.form.and_then(|form| form.schema_validation),
        Some(false)
    );
    assert!(elicitation.url.is_none());
    assert!(both.capabilities.sampling.is_some());
}
