//! Retained prompt inputs, separate from the behavioral text selected per model.

use std::path::{Path, PathBuf};

use crate::model_identity::PromptModel;

use super::{
    host_prompt::HostPrompt,
    model_prompts::{self, ModelPrompt, ModelPromptMode},
    project_instructions::ProjectInstructions,
    PromptSource, PromptSourceKind, SystemPrompt, BASE_SYSTEM_PROMPT,
};

/// Ordered assembly parts keep each retained section's text and provenance together.
#[derive(Clone)]
enum PromptPart {
    /// Rho's tool-usage guidance, which a host's override replaces.
    Guidance {
        text: String,
        sources: Vec<PromptSource>,
    },
    Retained {
        text: String,
        sources: Vec<PromptSource>,
    },
    ProjectInstructions(ProjectInstructions),
}

/// Snapshot of tool, skill and host context. Model switches reload only
/// model-specific behavioral text.
#[derive(Clone)]
pub(crate) struct ModelPromptTemplate {
    home: Option<PathBuf>,
    before_model: String,
    parts: Vec<PromptPart>,
    /// MCP servers, OpenAPI sources and the `ui` tool; see
    /// [`super::tool_sources_context`].
    tool_sources: String,
    /// The host's instructions, if it gave any.
    host: Option<HostPrompt>,
}

#[cfg(test)]
#[path = "model_prompt_template_tests.rs"]
mod tests;

impl ModelPromptTemplate {
    /// A template with Rho's tool-usage `guidance` and its `sources`.
    pub(super) fn new(
        home: Option<&Path>,
        before_model: String,
        guidance: String,
        sources: Vec<PromptSource>,
    ) -> Self {
        Self {
            home: home.map(Path::to_path_buf),
            before_model,
            parts: vec![PromptPart::Guidance {
                text: guidance,
                sources,
            }],
            tool_sources: String::new(),
            host: None,
        }
    }

    /// Puts `context`, which describes the session rather than guides the
    /// model, right before the guidance, where a host's override keeps it.
    pub(super) fn with_context_before_guidance(mut self, context: String) -> Self {
        if !context.is_empty() {
            let sources = vec![PromptSource {
                kind: PromptSourceKind::Base,
                path: None,
                bytes: context.len(),
            }];
            self.parts.insert(
                0,
                PromptPart::Retained {
                    text: context,
                    sources,
                },
            );
        }
        self
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
                PromptPart::Guidance { .. } | PromptPart::Retained { .. } => None,
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

    /// Sets the host's instructions, which every later render applies.
    pub(crate) fn set_host_prompt(&mut self, host: Option<HostPrompt>) {
        self.host = host;
    }

    /// Replace startup external tool context rather than retaining stale
    /// connect status.
    pub(crate) fn replace_tool_sources(
        &mut self,
        mcp: &crate::tools::mcp::McpSessionReport,
        openapi: &crate::tools::openapi::report::OpenApiReport,
        ui: super::UiTool,
    ) {
        self.tool_sources = super::tool_sources_context(mcp, openapi, ui);
    }

    /// A model switch preserves instruction files cached for the current session.
    /// A host's override reads no model prompt file, which it would drop.
    pub(crate) fn build(&self, running: &PromptModel) -> anyhow::Result<SystemPrompt> {
        let selected = match self.host.as_ref().and_then(HostPrompt::override_text) {
            Some(_) => None,
            None => model_prompts::load(self.home.as_deref(), running)?,
        };
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
        let host_override = self.host.as_ref().and_then(HostPrompt::override_text);
        // The host's override stands in for the base and model prompts.
        let selected = selected.filter(|_| host_override.is_none());
        if let Some(host) = host_override {
            text.push_str(host);
            sources.push(PromptSource {
                kind: PromptSourceKind::Host,
                path: None,
                bytes: host.len(),
            });
        } else if !matches!(
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
        text.push_str(&self.tool_sources);
        sources[0].bytes += text.len() - start;
        for part in &self.parts {
            match part {
                PromptPart::Guidance { .. } if host_override.is_some() => {}
                PromptPart::Guidance {
                    text: retained,
                    sources: retained_sources,
                }
                | PromptPart::Retained {
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
        if let Some(section) = self.host.as_ref().and_then(HostPrompt::appended_section) {
            text.push_str(&section);
            sources.push(PromptSource {
                kind: PromptSourceKind::Host,
                path: None,
                bytes: section.len(),
            });
        }
        SystemPrompt {
            text,
            sources,
            model_prompt: selected.cloned(),
        }
    }
}
