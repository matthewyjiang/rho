use crate::{
    agent::{
        effective_internal_agent_reasoning, internal_agent_accepts_claude_runtime,
        internal_agent_requires_model, AgentCatalog, AgentCatalogEntry, AgentOrigin,
        AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy,
    },
    config::{InternalAgentModelConfig, InternalAgentTarget},
};

use super::{
    model_picker::{ClaudeCodeRows, ConversationModelRow, InternalAgentSelection},
    picker::{DetailBlock, DetailField, DetailSheet, DetailTone, OverlayChrome},
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
    /// Opened by `/permissions auto`; selection applies Auto and returns to input.
    PermissionModeCommand,
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
        matches!(
            self,
            Self::AdvisorCommand | Self::PermissionModeStartup | Self::PermissionModeCommand
        )
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

/// Wrapped rows of prompt text the collapsed detail shows before Enter opens
/// the full prompt. Sized so the fact sheet plus excerpt fits a 24-row
/// terminal's detail pane.
const PROMPT_EXCERPT_ROWS: usize = 3;

pub(super) fn agent_picker(catalog: AgentCatalog, models: AgentModelView<'_>) -> UiPicker {
    let mut entries = catalog.iter_with_internal().collect::<Vec<_>>();
    // Group by source (stable within a group) so section headers answer
    // "which of these can I change?" at a glance.
    entries.sort_by_key(|entry| AgentGroup::of(entry.metadata.origin));
    let items = entries
        .into_iter()
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

/// Nav sections, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum AgentGroup {
    Editable,
    Workflow,
    Shared,
    BuiltIn,
    Internal,
}

impl AgentGroup {
    fn of(origin: AgentOrigin) -> Self {
        match origin {
            AgentOrigin::RhoHome | AgentOrigin::Project => Self::Editable,
            AgentOrigin::Workflow => Self::Workflow,
            AgentOrigin::AgentsHome => Self::Shared,
            AgentOrigin::BuiltIn => Self::BuiltIn,
            AgentOrigin::Internal => Self::Internal,
        }
    }

    fn section(self) -> &'static str {
        match self {
            Self::Editable => "YOURS",
            Self::Workflow => "WORKFLOW",
            Self::Shared => "SHARED",
            Self::BuiltIn => "BUILT IN",
            Self::Internal => "INTERNAL",
        }
    }
}

fn agent_item(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> PickerItem {
    let definition = &entry.definition;
    let selection_verb = match entry.metadata.origin {
        AgentOrigin::Internal => Some("configure"),
        AgentOrigin::RhoHome | AgentOrigin::Project => Some("edit"),
        AgentOrigin::BuiltIn | AgentOrigin::AgentsHome | AgentOrigin::Workflow => {
            Some("view prompt")
        }
    };
    PickerItem {
        section: Some(AgentGroup::of(entry.metadata.origin).section().into()),
        label: definition.id.to_string(),
        detail: Some(agent_detail(entry, models).into()),
        preview: None,
        badge: Some(agent_badge(entry.metadata.origin)),
        value: definition.id.to_string(),
        selection_verb,
        allow_filter_completion: true,
    }
}

/// One-glyph origin marker for the nav list. Editable agents get the accent
/// so they stand out; everything else recedes.
fn agent_badge(origin: AgentOrigin) -> PickerBadge {
    let tone = match origin {
        AgentOrigin::RhoHome | AgentOrigin::Project => PickerBadgeTone::Editable,
        AgentOrigin::Internal => PickerBadgeTone::Warning,
        AgentOrigin::BuiltIn | AgentOrigin::AgentsHome | AgentOrigin::Workflow => {
            PickerBadgeTone::Muted
        }
    };
    PickerBadge {
        text: "●".into(),
        tone,
    }
}

/// Right-aligned tag on the detail title: what the user may do with it.
fn agent_access_tag(origin: AgentOrigin) -> PickerBadge {
    let (text, tone) = match origin {
        AgentOrigin::RhoHome | AgentOrigin::Project => ("● editable", PickerBadgeTone::Editable),
        AgentOrigin::Internal => ("● internal", PickerBadgeTone::Warning),
        AgentOrigin::BuiltIn | AgentOrigin::AgentsHome | AgentOrigin::Workflow => {
            ("● read-only", PickerBadgeTone::Muted)
        }
    };
    PickerBadge {
        text: text.into(),
        tone,
    }
}

fn agent_detail(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> DetailSheet {
    let definition = &entry.definition;
    let origin = entry.metadata.origin;
    let mut fields = vec![
        DetailField::new(
            "Runtime",
            definition.runtime.runtime().to_string(),
            DetailTone::Normal,
        ),
        agent_model_field(entry, models),
        agent_reasoning_field(entry, models),
        agent_tools_field(definition),
    ];
    if let AgentRuntimeSpec::ClaudeCli(config) = &definition.runtime {
        fields.push(DetailField::new(
            "Claude config",
            if config.inherit_claude_config {
                "inherit"
            } else {
                "closed"
            },
            if config.inherit_claude_config {
                DetailTone::Normal
            } else {
                DetailTone::Muted
            },
        ));
    }
    fields.push(agent_source_field(entry));

    let mut blocks = vec![DetailBlock::Title {
        text: definition.id.to_string(),
        tag: Some(agent_access_tag(origin)),
    }];
    if !definition.description.is_empty() {
        blocks.push(DetailBlock::Paragraph(definition.description.to_string()));
    }
    blocks.push(DetailBlock::Rule);
    blocks.push(DetailBlock::Fields(fields));
    blocks.push(DetailBlock::Rule);
    blocks.extend(agent_prompt_blocks(definition));
    DetailSheet { blocks }
}

fn agent_model_field(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> DetailField {
    let definition = &entry.definition;
    if entry.metadata.origin == AgentOrigin::Internal {
        return match models.internal_agents.get(definition.id.as_str()) {
            Some(selection) => {
                DetailField::new("Model", selection.display_reference(), DetailTone::Normal)
                    .with_note("override")
            }
            None if internal_agent_requires_model(definition.id.as_str()) => {
                DetailField::new("Model", "not selected", DetailTone::Warning)
                    .with_note("no conversation fallback")
            }
            None => DetailField::new(
                "Model",
                rho_providers::provider::model_reference(models.provider, models.model),
                DetailTone::Muted,
            )
            .with_note("conversation model"),
        };
    }
    match definition.model_policy().as_ref() {
        ModelPolicy::Inherit => DetailField::new("Model", "inherit", DetailTone::Muted),
        ModelPolicy::Prefer(selection) => {
            DetailField::new("Model", model_name(selection), DetailTone::Normal).with_note("prefer")
        }
        ModelPolicy::Require(selection) => {
            DetailField::new("Model", model_name(selection), DetailTone::Normal)
                .with_note("require")
        }
        ModelPolicy::Select(selection) => {
            DetailField::new("Model", model_name(selection), DetailTone::Normal)
        }
    }
}

fn agent_reasoning_field(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> DetailField {
    let definition = &entry.definition;
    let configured = (entry.metadata.origin == AgentOrigin::Internal)
        .then(|| models.internal_agents.get(definition.id.as_str()))
        .flatten();
    let level = match configured {
        Some(selection) => {
            Some(effective_internal_agent_reasoning(definition.id.as_str(), selection).to_string())
        }
        None => definition.reasoning().map(|level| level.to_string()),
    };
    match level {
        Some(level) => DetailField::new("Reasoning", level, DetailTone::Normal),
        None => DetailField::new("Reasoning", "inherit", DetailTone::Muted),
    }
}

fn agent_tools_field(definition: &crate::agent::AgentDefinition) -> DetailField {
    let summary = definition.tools_summary();
    let tone = if summary == "none" {
        DetailTone::Muted
    } else {
        DetailTone::Normal
    };
    DetailField::new("Tools", summary, tone)
}

/// The file path when there is one (the nav section already names the
/// source kind), kept to one row with its tail visible.
fn agent_source_field(entry: &AgentCatalogEntry) -> DetailField {
    match entry.metadata.path.as_deref() {
        Some(path) => {
            DetailField::new("Source", crate::paths::display(path), DetailTone::Normal).keep_end()
        }
        None => DetailField::new("Source", "embedded in rho", DetailTone::Muted),
    }
}

/// Collapsed prompt: a heading with the policy and size, then a short
/// excerpt. The full text lives behind Enter (editor or read-only view).
fn agent_prompt_blocks(definition: &crate::agent::AgentDefinition) -> Vec<DetailBlock> {
    let (policy, text) = match &definition.prompt {
        PromptPolicy::Extend(text) => ("extends system prompt", text.as_str()),
        PromptPolicy::Replace(text) => ("replaces system prompt", text.as_str()),
    };
    let status = match text.lines().count() {
        0 => policy.to_string(),
        1 => format!("{policy} · 1 line"),
        lines => format!("{policy} · {lines} lines"),
    };
    let mut blocks = vec![DetailBlock::Heading {
        label: "PROMPT".into(),
        status,
    }];
    if text.trim().is_empty() {
        blocks.push(DetailBlock::Muted("(no prompt body)".into()));
        return blocks;
    }
    blocks.push(DetailBlock::Excerpt {
        text: text.to_string(),
        rows: PROMPT_EXCERPT_ROWS,
    });
    blocks
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

#[cfg(test)]
#[path = "agent_picker_tests.rs"]
mod tests;
