//! Retained prompt inputs, separate from the behavioral text selected per model.

use std::path::{Path, PathBuf};

use crate::model_identity::PromptModel;

use super::{
    model_prompts::{self, ModelPrompt, ModelPromptMode},
    PromptSource, PromptSourceKind, SystemPrompt, BASE_SYSTEM_PROMPT,
};

/// Snapshot of session instructions. Only model identity and model-specific
/// behavioral text change on a switch; tool contracts and project context stay.
#[derive(Clone)]
pub(crate) struct ModelPromptTemplate {
    home: Option<PathBuf>,
    before_model: String,
    retained: String,
    sources: Vec<PromptSource>,
}

#[cfg(test)]
#[path = "model_prompt_template_tests.rs"]
mod tests;

impl ModelPromptTemplate {
    pub(super) fn new(
        home: Option<&Path>,
        before_model: String,
        retained: String,
        sources: Vec<PromptSource>,
    ) -> Self {
        Self {
            home: home.map(Path::to_path_buf),
            before_model,
            retained,
            sources,
        }
    }

    /// Add host-owned instructions that model replacement must never remove.
    pub(crate) fn append_retained(&mut self, text: &str) {
        self.retained.push_str(text);
        self.sources[0].bytes += text.len();
    }

    /// Read current model prompt files only at explicit lifecycle boundaries.
    pub(crate) fn build(&self, running: &PromptModel) -> anyhow::Result<SystemPrompt> {
        let selected = model_prompts::load(self.home.as_deref(), running)?;
        Ok(self.render(running, selected.as_ref()))
    }

    /// Refresh runtime labels using the already-loaded model prompt. Catalog or
    /// MCP startup hydration must not unexpectedly reload user-authored files.
    pub(crate) fn render(
        &self,
        running: &PromptModel,
        selected: Option<&ModelPrompt>,
    ) -> SystemPrompt {
        let mut text = String::new();
        let mut sources = self.sources.clone();
        if !matches!(
            selected.map(|prompt| prompt.mode),
            Some(ModelPromptMode::Replace)
        ) {
            text.push_str(BASE_SYSTEM_PROMPT);
            sources[0].bytes += BASE_SYSTEM_PROMPT.len();
        }
        if let Some(selected) = selected {
            let start = text.len();
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&selected.body);
            sources.insert(
                1,
                PromptSource {
                    kind: match selected.mode {
                        ModelPromptMode::Append => PromptSourceKind::ModelAppend,
                        ModelPromptMode::Replace => PromptSourceKind::ModelReplace,
                    },
                    path: Some(selected.path.display().to_string()),
                    bytes: text.len() - start,
                },
            );
        }
        let start = text.len();
        text.push_str(&self.before_model);
        text.push_str(&format!(
            "You are running on {}. Rho can switch this mid-session and tells you when it does.\n",
            running.describe(),
        ));
        sources[0].bytes += text.len() - start;
        text.push_str(&self.retained);
        SystemPrompt {
            text,
            sources,
            model_prompt: selected.cloned(),
        }
    }
}
