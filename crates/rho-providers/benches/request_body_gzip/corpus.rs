//! Synthetic coding history made only from committed, public Rust source.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io, path::Path, process::Command};

// Fixed inventory, rather than whichever files happen to exist on the machine.
const SOURCES: [&str; 40] = [
    "crates/rho-providers/src/protocol/openai_shared/codex_sse.rs",
    "crates/rho-providers/src/provider.rs",
    "crates/rho-sdk/src/orchestration.rs",
    "crates/rho-providers/src/providers/openai/codex_ws.rs",
    "crates/rho-providers/src/model/models_dev.rs",
    "crates/rho-sdk/src/session.rs",
    "crates/rho-sdk/src/orchestration/tool_batch/coordinator.rs",
    "crates/rho-providers/src/protocol/openai_shared/convert.rs",
    "crates/rho-providers/src/auth/codex_oauth.rs",
    "crates/rho-providers/src/model/provider_models.rs",
    "crates/rho-providers/src/providers/sdk_contract.rs",
    "crates/rho-sdk/src/model.rs",
    "crates/rho-providers/src/providers/tui_fixture/stream_scenarios.rs",
    "crates/rho-sdk/src/client.rs",
    "crates/rho-sdk/src/provider.rs",
    "crates/rho-providers/src/provider_table.rs",
    "crates/rho-sdk/src/orchestration/async_jobs.rs",
    "crates/rho-providers/src/protocol/anthropic_messages/stream.rs",
    "crates/rho-providers/src/providers/openai/mod.rs",
    "crates/rho-providers/src/protocol/gemini_generate_content/convert.rs",
    "crates/rho-providers/src/auth/login_dispatch.rs",
    "crates/rho-sdk/src/hooks/payload.rs",
    "crates/rho-sdk/src/compaction.rs",
    "crates/rho-providers/src/credentials/mod.rs",
    "crates/rho-sdk/src/decision.rs",
    "crates/rho-providers/src/model/catalog.rs",
    "crates/rho-providers/src/auth/github_copilot_token.rs",
    "crates/rho-providers/src/auth/provider_credentials.rs",
    "crates/rho-providers/src/protocol/anthropic_messages/convert.rs",
    "crates/rho-providers/src/auth/xai_oauth.rs",
    "crates/rho-sdk/src/tool_host.rs",
    "crates/rho-providers/src/providers/builder.rs",
    "crates/rho-sdk/src/workspace/approval.rs",
    "crates/rho-providers/src/model/models_dev_document_de.rs",
    "crates/rho-providers/src/providers/tui_fixture.rs",
    "crates/rho-providers/src/auth/github_copilot_device.rs",
    "crates/rho-sdk/src/hooks/envelope.rs",
    "crates/rho-providers/src/model/models_dev_hydrate.rs",
    "crates/rho-sdk/src/tool.rs",
    "crates/rho-providers/src/providers/anthropic/mod.rs",
];

pub struct Corpus {
    sources: Vec<String>,
    pub revision: String,
}

fn git(root: &Path, args: &[&str]) -> io::Result<String> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

impl Corpus {
    pub fn load(revision: &str) -> io::Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let revision = git(
            &root,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{revision}^{{commit}}"),
            ],
        )?
        .trim()
        .to_owned();
        let sources = SOURCES
            .iter()
            .map(|path| git(&root, &["show", &format!("{revision}:{path}")]))
            .collect::<io::Result<_>>()?;
        Ok(Self { sources, revision })
    }

    /// Fit source prefixes into a valid JSON request, never truncate JSON/UTF-8.
    /// Small snapshots use one user message; long snapshots have 40 messages
    /// (ten user / assistant-tool-call / tool-result / assistant cycles).
    pub fn body(&self, target: usize) -> Vec<u8> {
        let long_history = target >= 16 * 1024;
        let mut low = 0;
        let mut high = self.sources.iter().map(String::len).max().unwrap();
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if self.serialize(mid, long_history).len() <= target {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        self.serialize(low, long_history)
    }

    fn snippet(&self, index: usize, budget: usize) -> String {
        let source = &self.sources[index];
        let mut end = source.len().min(budget);
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n```rust\n{}\n```", SOURCES[index], &source[..end])
    }

    fn serialize(&self, budget: usize, long_history: bool) -> Vec<u8> {
        let messages: Vec<Value> = if long_history {
            (0..10)
                .flat_map(|turn| {
                    let index = turn * 4;
                    let id = format!("read_{turn}");
                    [
                        json!({"role":"user", "content": format!(
                            "Review cancellation and error ownership here; preserve public contracts.\n{}",
                            self.snippet(index, budget))}),
                        json!({"role":"assistant", "content":[
                            {"type":"text", "text":format!(
                                "I’ll compare the caller before choosing a fix.\n{}",
                                self.snippet(index + 1, budget))},
                            {"type":"tool_use", "id":id, "name":"read_file",
                             "input":{"path":SOURCES[index + 2]}}
                        ]}),
                        json!({"role":"user", "content":[{"type":"tool_result",
                            "tool_use_id":id, "content":self.snippet(index + 2, budget)}]}),
                        json!({"role":"assistant", "content":format!(
                            "The boundary must propagate errors without losing context. Compare this implementation; then validate the narrow owner layer.\n{}",
                            self.snippet(index + 3, budget))}),
                    ]
                })
                .collect()
        } else {
            vec![json!({"role":"user", "content":format!(
                "Review this implementation for cancellation safety:\n{}", self.snippet(0, budget))})]
        };
        serde_json::to_vec(&json!({
            "model":"claude-sonnet-4-5", "max_tokens":4096, "stream":true,
            "system":"You are a coding assistant reviewing public Rust source. Keep changes focused, explain ownership, and preserve UTF-8: café, 日本語, naïve → correct. No private sessions or credentials are included.",
            "tools":[
                {"name":"read_file", "description":"Read a repository source file",
                 "input_schema":{"type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]}},
                {"name":"apply_patch", "description":"Apply a focused source patch",
                 "input_schema":{"type":"object", "properties":{"patch":{"type":"string"}}, "required":["patch"]}}
            ],
            "messages":messages,
        })).unwrap()
    }
}

pub fn sha256(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}
