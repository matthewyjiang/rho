//! Retained prompt inputs, separate from the behavioral text selected per model.

use std::path::{Path, PathBuf};

use crate::model_identity::PromptModel;

use super::{
    model_prompts::{self, ModelPrompt, ModelPromptMode},
    project_instructions::ProjectInstructions,
    PromptSource, PromptSourceKind, SystemPrompt, BASE_SYSTEM_PROMPT,
};

/// Snapshot of session instructions. Only model identity and model-specific
/// behavioral text change on a switch; /new also reloads AGENTS.md context.
#[derive(Clone)]
pub(crate) struct ModelPromptTemplate {
    home: Option<PathBuf>,
    before_model: String,
    retained: String,
    project_instructions: Option<ProjectInstructions>,
    mcp: String,
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
            project_instructions: None,
            mcp: String::new(),
            sources,
        }
    }

    pub(super) fn with_project_instructions(mut self, cwd: &Path, retained_offset: usize) -> Self {
        self.project_instructions = Some(ProjectInstructions::new(
            cwd,
            self.home.as_deref(),
            retained_offset,
        ));
        self
    }

    /// /new refreshes instruction files without rebuilding tool or skill context.
    pub(crate) fn reload_project_instructions(&mut self) {
        if let Some(instructions) = &mut self.project_instructions {
            instructions.reload(self.home.as_deref());
        }
    }

    /// Add host-owned instructions that model replacement must never remove.
    pub(crate) fn append_retained(&mut self, text: &str) {
        self.retained.push_str(text);
        self.sources[0].bytes += text.len();
    }

    /// Replace startup MCP context rather than retaining stale connect status.
    pub(crate) fn replace_mcp(&mut self, report: &crate::tools::mcp::McpSessionReport) {
        self.mcp = super::mcp_context(report);
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
        text.push_str(&self.mcp);
        if let Some(instructions) = &self.project_instructions {
            let (prefix, suffix) = self.retained.split_at(instructions.retained_offset);
            text.push_str(prefix);
            instructions.append_to(&mut text, &mut sources);
            text.push_str(suffix);
        } else {
            text.push_str(&self.retained);
        }
        sources[0].bytes += self.mcp.len();
        SystemPrompt {
            text,
            sources,
            model_prompt: selected.cloned(),
        }
    }
}
