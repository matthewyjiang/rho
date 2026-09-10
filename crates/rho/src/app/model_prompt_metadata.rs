//! Host-owned model prompt provenance, persisted without changing SDK contracts.

use crate::prompt::model_prompts::ModelPrompt;
use rho_sdk::SessionSnapshot;

pub(super) const KEY: &str = "rho.model_prompt";

/// Persisted identity is not a loaded file: it contains no body and must never
/// be used to render an overlay. Paths are display strings, including on Unix.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ModelPromptProvenance {
    pub path: String,
    pub mode: crate::prompt::model_prompts::ModelPromptMode,
    pub sha256: String,
}

impl From<&ModelPrompt> for ModelPromptProvenance {
    fn from(prompt: &ModelPrompt) -> Self {
        Self {
            path: prompt.path.display().to_string(),
            mode: prompt.mode,
            sha256: prompt.sha256.clone(),
        }
    }
}

pub(super) fn selection_notice(prompt: Option<&ModelPrompt>) -> String {
    provenance_notice(prompt.map(ModelPromptProvenance::from).as_ref())
}

pub(super) fn provenance_notice(prompt: Option<&ModelPromptProvenance>) -> String {
    match prompt {
        Some(prompt) => format!("model prompt: {} ({})", prompt.path, prompt.mode.as_str(),),
        None => "model prompt: default".into(),
    }
}

pub(super) fn encode(prompt: Option<&ModelPrompt>) -> String {
    serde_json::json!(prompt.map(ModelPromptProvenance::from)).to_string()
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

#[cfg(all(test, unix))]
#[path = "model_prompt_metadata_tests.rs"]
mod tests;
