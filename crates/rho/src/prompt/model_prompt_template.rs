//! Retained prompt inputs, separate from the behavioral text selected per model.

use std::path::{Path, PathBuf};

use crate::model_identity::PromptModel;

use super::{
    model_prompts::{self, ModelPrompt, ModelPromptMode},
    project_instructions::ProjectInstructions,
    PromptSource, PromptSourceKind, SystemPrompt, BASE_SYSTEM_PROMPT,
};

/// Instruction files remain cached within a session, including model switches.
#[derive(Clone, Copy)]
pub(crate) enum PromptSession {
    Current,
    Different,
}

/// Ordered assembly parts keep each retained section's text and provenance together.
#[derive(Clone)]
enum PromptPart {
    Retained {
        text: String,
        sources: Vec<PromptSource>,
    },
    ProjectInstructions(ProjectInstructions),
}

/// Snapshot of tool, skill and host context. Entering a different session reloads
/// AGENTS.md; model switches only reload model-specific behavioral text.
#[derive(Clone)]
pub(crate) struct ModelPromptTemplate {
    home: Option<PathBuf>,
    before_model: String,
    parts: Vec<PromptPart>,
    mcp: String,
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
            parts: vec![PromptPart::Retained {
                text: retained,
                sources,
            }],
            mcp: String::new(),
        }
    }

    pub(super) fn with_project_instructions(mut self, cwd: &Path) -> Self {
        self.parts
            .push(PromptPart::ProjectInstructions(ProjectInstructions::new(
                cwd,
                self.home.as_deref(),
            )));
        self
    }

    /// Sets the added workspace directories whose AGENTS.md files load with
    /// the project's, reloading instruction files. Returns files that were not
    /// loaded before so a live session can be told about them.
    pub(crate) fn set_added_dirs(
        &mut self,
        added_dirs: &crate::added_dirs::AddedDirs,
    ) -> Vec<(PathBuf, String)> {
        let home = self.home.clone();
        self.parts
            .iter_mut()
            .filter_map(|part| match part {
                PromptPart::ProjectInstructions(instructions) => Some(instructions),
                PromptPart::Retained { .. } => None,
            })
            .flat_map(|instructions| {
                instructions.set_added_dirs(added_dirs.as_slice(), home.as_deref())
            })
            .collect()
    }

    pub(super) fn append_section(&mut self, text: String, sources: Vec<PromptSource>) {
        self.parts.push(PromptPart::Retained { text, sources });
    }

    /// Add host-owned instructions that model replacement must never remove.
    pub(crate) fn append_retained(&mut self, text: &str) {
        self.append_section(
            text.to_owned(),
            vec![PromptSource {
                kind: PromptSourceKind::Base,
                path: None,
                bytes: text.len(),
            }],
        );
    }

    /// Replace startup MCP context rather than retaining stale connect status.
    pub(crate) fn replace_mcp(&mut self, report: &crate::tools::mcp::McpSessionReport) {
        self.mcp = super::mcp_context(report);
    }

    /// A model switch preserves instruction files cached for the current session.
    pub(crate) fn build(&self, running: &PromptModel) -> anyhow::Result<SystemPrompt> {
        let selected = model_prompts::load(self.home.as_deref(), running)?;
        Ok(self.render(running, selected.as_ref()))
    }

    /// Validate model prompts before refreshing instructions, then render once.
    /// Tools, skills, and host context are retained without rediscovery.
    pub(crate) fn build_for_session(
        &mut self,
        running: &PromptModel,
        session: PromptSession,
    ) -> anyhow::Result<SystemPrompt> {
        let selected = model_prompts::load(self.home.as_deref(), running)?;
        match session {
            PromptSession::Current => {}
            PromptSession::Different => {
                for part in &mut self.parts {
                    if let PromptPart::ProjectInstructions(instructions) = part {
                        instructions.reload(self.home.as_deref());
                    }
                }
            }
        }
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
        let mut sources = vec![PromptSource {
            kind: PromptSourceKind::Base,
            path: None,
            bytes: 0,
        }];
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
            sources.push(PromptSource {
                kind: match selected.mode {
                    ModelPromptMode::Append => PromptSourceKind::ModelAppend,
                    ModelPromptMode::Replace => PromptSourceKind::ModelReplace,
                },
                path: Some(selected.path.display().to_string()),
                bytes: text.len() - start,
            });
        }
        let start = text.len();
        text.push_str(&self.before_model);
        text.push_str(&format!(
            "You are running on {}. Rho can switch this mid-session and tells you when it does.\n",
            running.describe(),
        ));
        text.push_str(&self.mcp);
        sources[0].bytes += text.len() - start;
        for part in &self.parts {
            match part {
                PromptPart::Retained {
                    text: retained,
                    sources: retained_sources,
                } => {
                    text.push_str(retained);
                    for source in retained_sources {
                        if source.kind == PromptSourceKind::Base {
                            sources[0].bytes += source.bytes;
                        } else {
                            sources.push(source.clone());
                        }
                    }
                }
                PromptPart::ProjectInstructions(instructions) => {
                    instructions.append_to(&mut text, &mut sources);
                }
            }
        }
        SystemPrompt {
            text,
            sources,
            model_prompt: selected.cloned(),
        }
    }
}
