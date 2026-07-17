use crate::agent::{
    AgentCatalog, AgentCatalogEntry, AgentOrigin, ModelPolicy, ModelSelection, PromptPolicy,
    ToolPolicy,
};

use super::{PickerAction, PickerItem, PickerLayout, UiPicker};

pub(super) fn agent_picker(catalog: AgentCatalog) -> UiPicker {
    let items = catalog.iter().map(agent_item).collect();
    UiPicker::new(
        "loaded agents",
        "type regex filter, enter or esc closes",
        items,
        PickerAction::ViewAgent,
    )
    .with_layout(PickerLayout::MasterDetail)
}

fn agent_item(entry: &AgentCatalogEntry) -> PickerItem {
    let definition = &entry.definition;
    PickerItem {
        label: definition.id.to_string(),
        detail: Some(agent_detail(entry)),
        preview: None,
        badge: None,
        value: definition.id.to_string(),
    }
}

fn agent_detail(entry: &AgentCatalogEntry) -> String {
    let definition = &entry.definition;
    let source = match entry.metadata.origin {
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
    let model = match &definition.model {
        ModelPolicy::Inherit => "inherit".to_string(),
        ModelPolicy::Prefer(selection) => format!("prefer {}", model_name(selection)),
        ModelPolicy::Require(selection) => format!("require {}", model_name(selection)),
        ModelPolicy::Select(selection) => format!("select {}", model_name(selection)),
    };
    let reasoning = definition
        .reasoning
        .map(|level| level.to_string())
        .unwrap_or_else(|| "inherit".to_string());
    let tools = match &definition.tools {
        ToolPolicy::All => "all".to_string(),
        ToolPolicy::Allow(tools) if tools.is_empty() => "none".to_string(),
        ToolPolicy::Allow(tools) => tools.join(", "),
    };
    let prompt = match &definition.prompt {
        PromptPolicy::Extend(_) => "extend system prompt",
        PromptPolicy::Replace(_) => "replace system prompt",
    };

    format!(
        "Description\n{}\n\nSource\n{source}\n{path}\n\nModel\n{model}\n\nReasoning\n{reasoning}\n\nTools\n{tools}\n\nPrompt\n{prompt}",
        definition.description
    )
}

fn model_name(selection: &ModelSelection) -> String {
    selection
        .provider
        .as_ref()
        .map(|provider| format!("{provider}/{}", selection.model))
        .unwrap_or_else(|| selection.model.clone())
}

impl super::App {
    pub(super) fn execute_agents_command(&mut self) -> anyhow::Result<()> {
        let catalog = match AgentCatalog::discover(&self.info.cwd) {
            Ok(catalog) => catalog,
            Err(error) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "could not reload agents: {error}"
                )));
                self.status = "agent reload failed".into();
                return Ok(());
            }
        };
        self.composer = super::ComposerMode::Picker(agent_picker(catalog));
        self.status = "loaded agents".into();
        Ok(())
    }
}

#[cfg(test)]
#[path = "agent_picker_tests.rs"]
mod tests;
