//! `rho __acp-fixture-agent`: the scripted ACP agent on stdio, debug builds only.
//!
//! PTY end-to-end tests put a `cursor-agent` shim on PATH that execs this, so
//! the real delegated path (binding, spawn, fence, ACP driver, artifacts) runs
//! offline against a deterministic peer.

use agent_client_protocol::Stdio;

use crate::{
    acp_runtime::scripted_agent::{Script, ScriptedAgent},
    cli::AcpFixtureAgentArgs,
};

pub(super) async fn run(args: &AcpFixtureAgentArgs) -> anyhow::Result<()> {
    if std::env::var_os("RHO_TUI_TEST_MODE").as_deref() != Some(std::ffi::OsStr::new("matrix")) {
        anyhow::bail!(
            "__acp-fixture-agent is a test fixture; it runs only with RHO_TUI_TEST_MODE=matrix"
        );
    }
    let script: Script = serde_json::from_slice(&std::fs::read(&args.script)?)?;
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.journal)?;
    let (agent, _record, _signals) = ScriptedAgent::new(script);
    agent
        .with_journal(journal)
        .run(Stdio::new())
        .await
        .map_err(|error| anyhow::anyhow!("scripted ACP agent: {error}"))
}
