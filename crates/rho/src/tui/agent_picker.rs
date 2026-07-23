use crate::{
    agent::{
        AgentCatalog, AgentCatalogEntry, AgentOrigin, ModelPolicy, ModelSelection, PromptPolicy,
        ToolPolicy,
    },
    config::InternalAgentModelConfig,
};

use super::{
    picker_overlay::OverlayChrome, PickerAction, PickerBadge, PickerBadgeTone, PickerItem,
    PickerLayout, RuntimeModelView, UiPicker,
};

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
    UiPicker::new(
        "loaded agents",
        "type regex filter, enter configures internal agents or closes, esc closes",
        items,
        PickerAction::ViewAgent,
    )
    .with_layout(PickerLayout::Overlay)
    .with_overlay_chrome(OverlayChrome {
        nav_label: " AGENTS".into(),
        detail_label: Some(" DETAILS".into()),
        nav_keys_hint: "↑↓ agents".into(),
    })
}

fn agent_item(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> PickerItem {
    let definition = &entry.definition;
    PickerItem {
        section: None,
        label: definition.id.to_string(),
        detail: Some(agent_detail(entry, models)),
        preview: None,
        badge: (entry.metadata.origin == AgentOrigin::Internal).then_some(PickerBadge {
            text: "(internal)".to_string(),
            tone: PickerBadgeTone::Internal,
        }),
        value: definition.id.to_string(),
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
    };
    let path = entry
        .metadata
        .path
        .as_deref()
        .map(crate::paths::display)
        .unwrap_or_else(|| "embedded in rho".to_string());
    let model = if entry.metadata.origin == AgentOrigin::Internal {
        let (provider, model, source) = match models.internal_agents.get(definition.id.as_str()) {
            Some(selection) => (
                selection.provider.as_str(),
                selection.model.as_str(),
                "override",
            ),
            None => (models.provider, models.model, "conversation fallback"),
        };
        format!(
            "{}\nModel source: {source}",
            rho_providers::provider::model_reference(provider, model)
        )
    } else {
        match &definition.model {
            ModelPolicy::Inherit => "inherit".to_string(),
            ModelPolicy::Prefer(selection) => format!("prefer {}", model_name(selection)),
            ModelPolicy::Require(selection) => format!("require {}", model_name(selection)),
            ModelPolicy::Select(selection) => format!("select {}", model_name(selection)),
        }
    };
    let reasoning = definition
        .reasoning
        .map(|level| level.to_string())
        .unwrap_or_else(|| "inherit".to_string());
    let tools = match &definition.tools {
        ToolPolicy::All => "all".to_string(),
        ToolPolicy::Allow(tools) if tools.is_empty() => "none".to_string(),
        ToolPolicy::Allow(tools) => tools
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    };
    let prompt = match &definition.prompt {
        PromptPolicy::Extend(text) if text.is_empty() => "extend system prompt".to_string(),
        PromptPolicy::Extend(text) => {
            format!("extend system prompt\n\nPrompt extension\n{text}")
        }
        PromptPolicy::Replace(text) => {
            format!("replace system prompt\n\nReplacement prompt\n{text}")
        }
    };

    let restrictions = if entry.metadata.origin == AgentOrigin::Internal {
        "\n\nRestrictions\nreserved; cannot be overridden or delegated"
    } else {
        ""
    };

    format!(
        "Description\n{}\n\nPrompt\n{prompt}\n\nSource\n{source}\n{path}\n\nModel\n{model}\n\nReasoning\n{reasoning}\n\nTools\n{tools}{restrictions}",
        definition.description
    )
}

fn model_name(selection: &ModelSelection) -> String {
    selection
        .provider
        .as_ref()
        .map(|provider| rho_providers::provider::model_reference(provider, &selection.model))
        .unwrap_or_else(|| selection.model.clone())
}

impl super::App {
    pub(super) fn open_internal_agent_model_picker(&mut self, id: &str) {
        self.refresh_available_auths();
        let uses_conversation_model = !self.info.runtime.internal_agents.contains_key(id);
        let (provider, model, _auth) = self.internal_agent_model_selection(id);
        let picker = super::model_picker::internal_agent_model_picker(
            id,
            &provider,
            &model,
            uses_conversation_model,
            &self.info.runtime.favorite_models,
            &self.available_auths,
        );
        self.internal_agent_model_target = Some(id.to_string());
        self.open_child_picker(picker);
    }

    pub(super) fn open_selected_internal_agent_model_picker(&mut self, id: &str) -> bool {
        let internal = crate::agent::internal_definitions()
            .iter()
            .any(|definition| definition.id.as_str() == id);
        if internal {
            self.open_internal_agent_model_picker(id);
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
                self.status = "agent reload failed".into();
                return Ok(());
            }
        };
        let mut picker = agent_picker(catalog, AgentModelView::from(&self.info.runtime));
        if let Some(id) = self.internal_agent_model_target.as_deref() {
            Self::restore_picker_position(&mut picker, id, String::new());
        }
        self.composer = super::ComposerMode::Picker(picker);
        self.status = "loaded agents".into();
        Ok(())
    }
}

#[cfg(test)]
#[path = "agent_picker_tests.rs"]
mod tests;
