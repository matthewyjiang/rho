//! One host-owned prompt state. Loaded overlay bytes live only until explicit
//! replacement/recovery; durable recovery uses assembled history and metadata,
//! never a file whose contents may have changed since the snapshot was saved.

use rho_sdk::{model::Message, SessionSnapshot, SystemPrompt};

use crate::{
    diagnostics::RuntimeDiagnostics,
    prompt::{model_prompts::ModelPrompt, PromptSource},
    tools::advisor::AdvisorSessionStore,
};

use super::model_prompt_metadata::{ModelPromptProvenance, KEY};

const SOURCES_KEY: &str = "rho.prompt_sources";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ActivePrompt {
    pub system: SystemPrompt,
    pub provenance: Option<ModelPromptProvenance>,
    /// Only fresh startup hydration may re-render these cached bytes. A saved
    /// fingerprint is deliberately not reconstructed as a fictitious file.
    pub loaded: Option<ModelPrompt>,
    sources: Vec<PromptSource>,
}

pub(super) enum PromptTransition {
    Keep,
    Replace(crate::prompt::SystemPrompt),
    Restore,
}

impl ActivePrompt {
    pub fn new(
        system: SystemPrompt,
        loaded: Option<ModelPrompt>,
        sources: Vec<PromptSource>,
    ) -> Self {
        Self {
            system,
            provenance: loaded.as_ref().map(ModelPromptProvenance::from),
            loaded,
            sources,
        }
    }

    pub fn from_prepared(prompt: crate::prompt::SystemPrompt) -> Self {
        Self::new(
            SystemPrompt::Custom(prompt.text),
            prompt.model_prompt,
            prompt.sources,
        )
    }

    /// Keep host-added instructions and their source accounting together.
    pub fn append_retained(&mut self, text: &str) {
        if let SystemPrompt::Custom(system) = &mut self.system {
            system.push_str(text);
            if let Some(base) = self
                .sources
                .iter_mut()
                .find(|source| source.kind == crate::prompt::PromptSourceKind::Base)
            {
                base.bytes += text.len();
            }
        }
    }

    pub fn from_snapshot(snapshot: &SessionSnapshot) -> anyhow::Result<Self> {
        let system = match snapshot.history().first() {
            Some(Message::System(text)) => SystemPrompt::Custom(text.clone()),
            _ => SystemPrompt::None,
        };
        Ok(Self {
            system,
            provenance: snapshot
                .metadata()
                .get(KEY)
                .map(|encoded| serde_json::from_str(encoded))
                .transpose()?
                .flatten(),
            loaded: None,
            // Legacy snapshots have no source byte accounting. Unknown is
            // preferable to publishing the sources of a different prompt.
            sources: snapshot
                .metadata()
                .get(SOURCES_KEY)
                .map(|encoded| serde_json::from_str(encoded))
                .transpose()?
                .unwrap_or_default(),
        })
    }

    /// All hosts adopt through here so advisor context and diagnostics cannot
    /// retain the old executor prompt after history changes.
    pub fn adopt(
        &mut self,
        next: Self,
        diagnostics: &RuntimeDiagnostics,
        advisor: Option<&AdvisorSessionStore>,
    ) {
        next.install(diagnostics, advisor);
        *self = next;
    }

    pub fn install(&self, diagnostics: &RuntimeDiagnostics, advisor: Option<&AdvisorSessionStore>) {
        diagnostics.update_prompt_sources(self.sources.clone());
        if let Some(advisor) = advisor {
            advisor.bind_system_prompt(match &self.system {
                SystemPrompt::Custom(text) => Some(text.clone()),
                _ => None,
            });
        }
        if let Some(prompt) = &self.provenance {
            tracing::debug!(path = %prompt.path, mode = prompt.mode.as_str(), sha256 = %prompt.sha256, "installed model prompt");
        }
    }

    pub fn decorate(&self, snapshot: SessionSnapshot) -> SessionSnapshot {
        snapshot
            .with_metadata(KEY, serde_json::json!(self.provenance).to_string())
            .with_metadata(SOURCES_KEY, serde_json::json!(self.sources).to_string())
    }
}

#[cfg(test)]
#[path = "active_prompt_tests.rs"]
mod tests;
