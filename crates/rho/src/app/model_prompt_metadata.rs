//! Host-owned model prompt provenance, persisted without changing SDK contracts.

use crate::prompt::model_prompts::ModelPrompt;
use rho_sdk::SessionSnapshot;

const KEY: &str = "rho.model_prompt";

pub(super) fn selection_notice(prompt: Option<&ModelPrompt>) -> String {
    match prompt {
        Some(prompt) => format!(
            "model prompt: {} ({})",
            prompt.path.display(),
            prompt.mode.as_str(),
        ),
        None => "model prompt: default".into(),
    }
}

pub(super) fn encode(prompt: Option<&ModelPrompt>) -> String {
    prompt.map_or_else(
        || "null".into(),
        |prompt| {
            serde_json::json!({
                "path": prompt.path,
                "mode": prompt.mode.as_str(),
                "sha256": prompt.sha256,
            })
            .to_string()
        },
    )
}

pub(super) fn decorate(snapshot: SessionSnapshot, prompt: Option<&ModelPrompt>) -> SessionSnapshot {
    snapshot.with_metadata(KEY, encode(prompt))
}

pub(super) fn decorate_encoded(snapshot: SessionSnapshot, encoded: &str) -> SessionSnapshot {
    snapshot.with_metadata(KEY, encoded)
}

/// Old sessions without provenance have no known fingerprint to compare.
pub(super) fn change_notice(
    previous: &SessionSnapshot,
    current: Option<&ModelPrompt>,
) -> Option<String> {
    let previous = previous.metadata().get(KEY)?;
    (previous != &encode(current)).then(|| match current {
        Some(prompt) => format!(
            "model prompt changed since this session was saved: {} · {}",
            prompt.path.display(),
            prompt.mode.as_str(),
        ),
        None => {
            "model prompt changed since this session was saved: using the default prompt".into()
        }
    })
}
