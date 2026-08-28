use crate::{
    agent::{
        effective_internal_agent_reasoning, internal_agent_accepts_claude_runtime,
        internal_agent_requires_model, AgentCatalog, AgentCatalogEntry, AgentOrigin,
        AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy, ToolPolicy,
    },
    config::{InternalAgentModelConfig, InternalAgentTarget},
};

use super::{
    model_picker::{ClaudeCodeRows, ConversationModelRow, InternalAgentSelection},
    picker::OverlayChrome,
    ComposerMode, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, RuntimeModelView,
    UiPicker,
};

/// Where an internal-agent model picker was opened from, which decides what a
/// selection means and where the user lands afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InternalAgentModelPickerOrigin {
    /// Opened from the agents picker; reopen it after a selection.
    AgentsPicker,
    /// Opened by `/advisor on`; a selection also turns advisor mode on.
    AdvisorCommand,
    /// Opened from the config picker's advisor mode row when enabling without a
    /// model; a selection also turns advisor mode on and returns to config.
    AdvisorConfigRow,
    /// Opened from the config picker's advisor model row; returns to config
    /// without forcing advisor mode on.
    AdvisorModelConfigRow,
    /// Opened from the config picker's permission mode row when enabling Auto
    /// without a classifier model; selection also applies Auto and returns to
    /// config.
    PermissionModeConfigRow,
    /// Opened from the config picker's classifier model row; returns to config
    /// without forcing Auto on.
    PermissionClassifierModelConfigRow,
    /// Opened at interactive startup when permission mode is already Auto with
    /// no classifier model. Selection keeps Auto; cancel falls back to
    /// Supervised. Opens alone in the composer (no parent picker).
    PermissionModeStartup,
}

impl InternalAgentModelPickerOrigin {
    /// True when the picker replaces the composer alone (no parent to return to).
    pub(super) fn opens_standalone(self) -> bool {
        matches!(self, Self::AdvisorCommand | Self::PermissionModeStartup)
    }
}

/// The internal agent an open model or reasoning picker configures.
#[derive(Clone, Debug)]
pub(super) struct InternalAgentModelTarget {
    pub(super) id: String,
    pub(super) origin: InternalAgentModelPickerOrigin,
}

/// Picker inputs for one internal agent's current model.
struct InternalAgentPickerModel {
    current: InternalAgentSelection,
    conversation_model: ConversationModelRow,
}

pub(super) struct AgentModelView<'a> {
    provider: &'a str,
    model: &'a str,
    internal_agents: &'a std::collections::BTreeMap<String, InternalAgentModelConfig>,
}

impl<'a> From<&'a RuntimeModelView> for AgentModelView<'a> {
    fn from(runtime: &'a RuntimeModelView) -> Self {
        Self {
            provider: &runtime.provider,
            model: &runtime.model,
            internal_agents: &runtime.internal_agents,
        }
    }
}

#[cfg(test)]
impl<'a> From<&'a crate::config::Config> for AgentModelView<'a> {
    fn from(config: &'a crate::config::Config) -> Self {
        Self {
            provider: &config.provider,
            model: &config.model,
            internal_agents: &config.internal_agents,
        }
    }
}

pub(super) fn agent_picker(catalog: AgentCatalog, models: AgentModelView<'_>) -> UiPicker {
    let items = catalog
        .iter_with_internal()
        .map(|entry| agent_item(entry, &models))
        .collect();
    UiPicker::view_agent("Loaded agents", items)
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " AGENTS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ agents".into(),
        })
}

fn agent_item(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> PickerItem {
    let definition = &entry.definition;
    let selection_verb = match entry.metadata.origin {
        AgentOrigin::Internal => Some("configure"),
        AgentOrigin::RhoHome | AgentOrigin::Project => Some("edit"),
        AgentOrigin::BuiltIn | AgentOrigin::AgentsHome | AgentOrigin::Workflow => Some("close"),
    };
    PickerItem {
        section: None,
        label: definition.id.to_string(),
        detail: Some(agent_detail(entry, models)),
        preview: None,
        badge: agent_badge(entry.metadata.origin),
        value: definition.id.to_string(),
        selection_verb,
        allow_filter_completion: true,
    }
}

/// Badge for the agents picker: internal agents show "(internal)", editable
/// user agents (RhoHome or Project) show "(editable)", others have none.
fn agent_badge(origin: AgentOrigin) -> Option<PickerBadge> {
    match origin {
        AgentOrigin::Internal => Some(PickerBadge {
            text: "(internal)".to_string(),
            tone: PickerBadgeTone::Internal,
        }),
        AgentOrigin::RhoHome | AgentOrigin::Project => Some(PickerBadge {
            text: "(editable)".to_string(),
            tone: PickerBadgeTone::Editable,
        }),
        AgentOrigin::BuiltIn | AgentOrigin::AgentsHome | AgentOrigin::Workflow => None,
    }
}

fn agent_detail(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> String {
    let definition = &entry.definition;
    let source = match entry.metadata.origin {
        AgentOrigin::Internal => "internal".to_string(),
        AgentOrigin::BuiltIn => "built in".to_string(),
        AgentOrigin::AgentsHome => "~/.agents/agents".to_string(),
        AgentOrigin::RhoHome => "~/.rho/agents".to_string(),
        AgentOrigin::Project => "project".to_string(),
        AgentOrigin::Workflow => "workflow".to_string(),
    };
    let path = entry
        .metadata
        .path
        .as_deref()
        .map(crate::paths::display)
        .unwrap_or_else(|| "embedded in rho".to_string());
    let model = if entry.metadata.origin == AgentOrigin::Internal {
        match models.internal_agents.get(definition.id.as_str()) {
            Some(selection) => format!("{}\nModel source: override", selection.display_reference()),
            None if internal_agent_requires_model(definition.id.as_str()) => {
                "not selected\nModel source: none; this agent has no conversation fallback"
                    .to_string()
            }
            None => format!(
                "{}\nModel source: conversation fallback",
                rho_providers::provider::model_reference(models.provider, models.model)
            ),
        }
    } else {
        match definition.model_policy().as_ref() {
            ModelPolicy::Inherit => "inherit".to_string(),
            ModelPolicy::Prefer(selection) => format!("prefer {}", model_name(selection)),
            ModelPolicy::Require(selection) => format!("require {}", model_name(selection)),
            ModelPolicy::Select(selection) => format!("select {}", model_name(selection)),
        }
    };
    let reasoning = if entry.metadata.origin == AgentOrigin::Internal {
        match models.internal_agents.get(definition.id.as_str()) {
            Some(selection) => {
                effective_internal_agent_reasoning(definition.id.as_str(), selection).to_string()
            }
            None => definition
                .reasoning()
                .map(|level| level.to_string())
                .unwrap_or_else(|| "inherit".to_string()),
        }
    } else {
        definition
            .reasoning()
            .map(|level| level.to_string())
            .unwrap_or_else(|| "inherit".to_string())
    };
    let (tools, inherit_claude_config) = match &definition.runtime {
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            ..
        } => ("all".to_string(), None),
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(tools),
            ..
        } if tools.is_empty() => ("none".to_string(), None),
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(tools),
            ..
        } => (
            tools
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            None,
        ),
        AgentRuntimeSpec::ClaudeCli(config) => (
            if config.tools.as_slice().is_empty() {
                "none".to_string()
            } else {
                config.tools.as_slice().join(", ")
            },
            Some(if config.inherit_claude_config {
                "yes"
            } else {
                "no"
            }),
        ),
    };
    let runtime = definition.runtime.runtime().to_string();
    let prompt = match &definition.prompt {
        PromptPolicy::Extend(text) if text.is_empty() => "extend system prompt".to_string(),
        PromptPolicy::Extend(text) => {
            format!("extend system prompt\n\nPrompt extension\n{text}")
        }
        PromptPolicy::Replace(text) => {
            format!("replace system prompt\n\nReplacement prompt\n{text}")
        }
    };

    // Every internal agent is reserved, but only some may run on Claude Code,
    // so the line has to name this agent's own runtime freedom.
    let restrictions = match entry.metadata.origin {
        AgentOrigin::Internal if internal_agent_accepts_claude_runtime(definition.id.as_str()) => {
            "\n\nRestrictions\nreserved; cannot be overridden, and runs on rho or claude code"
        }
        AgentOrigin::Internal => "\n\nRestrictions\nreserved; cannot be overridden or delegated",
        AgentOrigin::BuiltIn
        | AgentOrigin::AgentsHome
        | AgentOrigin::RhoHome
        | AgentOrigin::Project
        | AgentOrigin::Workflow => "",
    };
    let inherit_section = inherit_claude_config
        .map(|value| format!("\n\nInherit Claude config\n{value}"))
        .unwrap_or_default();

    format!(
        "Description\n{}\n\nPrompt\n{prompt}\n\nSource\n{source}\n{path}\n\nRuntime\n{runtime}\n\nModel\n{model}\n\nReasoning\n{reasoning}\n\nTools\n{tools}{inherit_section}{restrictions}",
        definition.description
    )
}

/// Whether this agent's picker offers Claude Code.
///
/// Both halves must hold: the agent has to accept a delegated run, and the
/// `claude` binary has to be installed. Offering rows Rho cannot run would
/// move the failure to the first advisor call.
fn claude_code_rows_for(id: &str) -> ClaudeCodeRows {
    if internal_agent_accepts_claude_runtime(id)
        && crate::claude_runtime::executable::resolve().is_ok()
    {
        ClaudeCodeRows::Offered
    } else {
        ClaudeCodeRows::Omitted
    }
}

fn model_name(selection: &ModelSelection) -> String {
    selection
        .provider
        .as_ref()
        .map(|provider| rho_providers::provider::model_reference(provider, &selection.model))
        .unwrap_or_else(|| selection.model.clone())
}

impl super::App {
    /// Builds the model picker for an internal agent and records what a
    /// selection means. Callers place the picker themselves, because the agents
    /// and config pickers open it as a child while `/advisor` opens it alone.
    pub(super) fn internal_agent_model_picker(
        &mut self,
        id: &str,
        origin: InternalAgentModelPickerOrigin,
    ) -> UiPicker {
        self.refresh_available_auths();
        let current = self.internal_agent_picker_model(id);
        let scope = self.resolved_model_picker_scope();
        let picker = super::model_picker::internal_agent_model_picker(
            super::model_picker::InternalAgentPickerInputs {
                agent_id: id,
                current: current.current,
                conversation_model: current.conversation_model,
                claude_code: claude_code_rows_for(id),
                favorite_models: &self.info.runtime.favorite_models,
                available_auths: &self.available_auths,
                scope,
                keybindings: &self.info.runtime.keybindings,
            },
        );
        self.internal_agent_model_target = Some(InternalAgentModelTarget {
            id: id.to_string(),
            origin,
        });
        picker
    }

    /// Model shown as selected in an internal agent's picker, and whether the
    /// conversation-model row belongs there.
    fn internal_agent_picker_model(&self, id: &str) -> InternalAgentPickerModel {
        let requires_model = internal_agent_requires_model(id);
        match self.info.runtime.internal_agents.get(id) {
            Some(selection) => InternalAgentPickerModel {
                current: match &selection.target {
                    InternalAgentTarget::Rho(rho) => InternalAgentSelection::RhoModel {
                        provider: rho.provider.clone(),
                        model: rho.model.clone(),
                    },
                    InternalAgentTarget::ClaudeCli { model } => {
                        InternalAgentSelection::ClaudeCode {
                            model: model.clone(),
                        }
                    }
                },
                conversation_model: if requires_model {
                    ConversationModelRow::Omitted
                } else {
                    ConversationModelRow::Offered { selected: false }
                },
            },
            None if requires_model => InternalAgentPickerModel {
                current: InternalAgentSelection::Unset,
                conversation_model: ConversationModelRow::Omitted,
            },
            None => InternalAgentPickerModel {
                current: InternalAgentSelection::RhoModel {
                    provider: self.info.runtime.provider.clone(),
                    model: self.info.runtime.model.clone(),
                },
                conversation_model: ConversationModelRow::Offered { selected: true },
            },
        }
    }

    /// Opens the model picker for an internal agent, placed by origin: as a
    /// child when a parent picker waits underneath, alone in the composer for
    /// `/advisor on`. Reports whether it opened; with no cached models it
    /// names the fix instead of showing an empty list.
    pub(super) fn open_internal_agent_model_picker(
        &mut self,
        id: &str,
        origin: InternalAgentModelPickerOrigin,
    ) -> bool {
        let picker = self.internal_agent_model_picker(id, origin);
        if picker.items.is_empty() {
            self.internal_agent_model_target = None;
            self.report_missing_cached_provider_models();
            return false;
        }
        if origin.opens_standalone() {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        } else {
            self.open_child_picker(picker)
        }
        true
    }

    pub(super) fn open_selected_internal_agent_model_picker(&mut self, id: &str) -> bool {
        let internal = crate::agent::internal_definitions()
            .iter()
            .any(|definition| definition.id.as_str() == id);
        if internal {
            self.open_internal_agent_model_picker(id, InternalAgentModelPickerOrigin::AgentsPicker);
        }
        internal
    }

    pub(super) fn execute_agents_command(&mut self) -> anyhow::Result<()> {
        let catalog = match AgentCatalog::discover(&self.info.runtime.cwd) {
            Ok(catalog) => catalog,
            Err(error) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "could not reload agents: {error}"
                )));
                self.input_ui.set_composer(super::ComposerMode::Input);
                self.set_status("agent reload failed");
                return Ok(());
            }
        };
        let mut picker = agent_picker(catalog, AgentModelView::from(&self.info.runtime));
        if let Some(target) = self.internal_agent_model_target.as_ref() {
            Self::restore_picker_position(&mut picker, &target.id, String::new());
        }
        self.input_ui
            .set_composer(super::ComposerMode::Picker(picker));
        self.set_status("loaded agents");
        Ok(())
    }
}
