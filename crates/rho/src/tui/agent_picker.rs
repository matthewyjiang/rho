use crate::{
    agent::{
        effective_internal_agent_reasoning, internal_agent_accepts_claude_runtime,
        internal_agent_requires_model, AgentCatalog, AgentCatalogEntry, AgentOrigin,
        AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy,
    },
    app::interactive_runtime::InteractiveRuntime,
    config::{InternalAgentModelConfig, InternalAgentTarget},
};

use super::{
    model_picker::{ClaudeCodeRows, FallbackModelRow, InternalAgentSelection},
    picker::{DetailBlock, DetailField, DetailSheet, DetailTone, ExcerptAnchor, OverlayChrome},
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
    /// Opened by `/advisor model`; a selection keeps the mode as it is and
    /// follows up with a reasoning picker when the model supports one.
    AdvisorModelCommand,
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
    /// The permission screen's picker, opened from its config row; returns to
    /// config.
    PermissionScreenModelConfigRow,
    /// The permission screen's picker, opened by Auto setup from the config
    /// picker's permission mode row once a classifier model is picked. Esc
    /// keeps the classifier model; either way returns to config.
    PermissionScreenSetupConfigRow,
    /// The permission screen's picker, opened by `/permissions auto` once a
    /// classifier model is picked. Opens alone in the composer.
    PermissionScreenSetupCommand,
}

impl InternalAgentModelPickerOrigin {
    /// True when the picker replaces the composer alone (no parent to return to).
    pub(super) fn opens_standalone(self) -> bool {
        matches!(
            self,
            Self::AdvisorCommand
                | Self::AdvisorModelCommand
                | Self::PermissionModeStartup
                | Self::PermissionModeCommand
                | Self::PermissionScreenSetupCommand
        )
    }
}

/// What an agent runs once its `[internal_agents.<id>]` entry is cleared.
#[derive(Clone, Debug)]
pub(super) enum AgentModelDefault {
    /// An internal agent: the conversation model, unless it needs its own.
    Internal,
    /// A read-only catalog agent (a plugin agent): the model its file names.
    Definition(ModelPolicy),
}

/// The agent an open model or reasoning picker configures.
#[derive(Clone, Debug)]
pub(super) struct AgentModelTarget {
    pub(super) id: String,
    pub(super) origin: InternalAgentModelPickerOrigin,
    pub(super) default: AgentModelDefault,
}

/// Picker inputs for one agent's current model.
struct AgentPickerModel {
    current: InternalAgentSelection,
    fallback: FallbackModelRow,
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
    // Stable, so catalog order holds within a section.
    entries.sort_by_key(|entry| agent_section(entry.metadata.origin).0);
    let items = entries
        .into_iter()
        .map(|entry| agent_item(entry, &models))
        .collect();
    UiPicker::view_agent("Loaded agents", items)
        .with_restore_status("loaded agents")
        .with_key_hints(super::PickerKeyHints {
            row_delete: super::RowDeleteKeys::DeleteOnly,
            ..Default::default()
        })
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " AGENTS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ agents".into(),
        })
}

/// What the user may do with an agent. One source of truth for the nav
/// section, the Enter verb, and the origin markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentAccess {
    Editable,
    Internal,
    /// A read-only file whose model the user may still pick in `/agents`,
    /// stored as an `[internal_agents.<id>]` override.
    ModelOnly,
    ReadOnly,
}

impl AgentAccess {
    pub(super) fn of(entry: &AgentCatalogEntry) -> Self {
        match entry.metadata.origin {
            AgentOrigin::RhoHome | AgentOrigin::Project => Self::Editable,
            AgentOrigin::Internal => Self::Internal,
            // Only Rho agents take a model override at bind.
            AgentOrigin::UserPlugin | AgentOrigin::ProjectPlugin => {
                match entry.definition.runtime {
                    AgentRuntimeSpec::Rho { .. } => Self::ModelOnly,
                    AgentRuntimeSpec::ClaudeCli(_)
                    | AgentRuntimeSpec::Cursor(_)
                    | AgentRuntimeSpec::Antigravity(_) => Self::ReadOnly,
                }
            }
            AgentOrigin::BuiltIn | AgentOrigin::AgentsHome | AgentOrigin::Workflow => {
                Self::ReadOnly
            }
        }
    }

    fn selection_verb(self) -> &'static str {
        match self {
            Self::Editable => "edit",
            Self::Internal | Self::ModelOnly => "configure",
            Self::ReadOnly => "view prompt",
        }
    }

    fn tone(self) -> PickerBadgeTone {
        match self {
            Self::Editable => PickerBadgeTone::Editable,
            Self::Internal | Self::ModelOnly => PickerBadgeTone::Warning,
            Self::ReadOnly => PickerBadgeTone::Muted,
        }
    }

    /// One-glyph nav marker; editable agents get the accent.
    fn marker(self) -> PickerBadge {
        PickerBadge {
            text: "●".into(),
            tone: self.tone(),
        }
    }

    /// Right-aligned tag on the detail title.
    fn tag(self) -> PickerBadge {
        let text = match self {
            Self::Editable => "● editable",
            Self::Internal => "● internal",
            Self::ModelOnly => "● model only",
            Self::ReadOnly => "● read-only",
        };
        PickerBadge {
            text: text.into(),
            tone: self.tone(),
        }
    }
}

/// Nav section and its display rank. Sorting by rank groups the catalog so
/// headers answer "which of these can I change?" at a glance.
fn agent_section(origin: AgentOrigin) -> (u8, &'static str) {
    match origin {
        AgentOrigin::RhoHome | AgentOrigin::Project => (0, "YOURS"),
        AgentOrigin::Workflow => (1, "WORKFLOW"),
        AgentOrigin::AgentsHome => (2, "SHARED"),
        AgentOrigin::UserPlugin | AgentOrigin::ProjectPlugin => (3, "PLUGIN"),
        AgentOrigin::BuiltIn => (4, "BUILT IN"),
        AgentOrigin::Internal => (5, "INTERNAL"),
    }
}

fn agent_item(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> PickerItem {
    let definition = &entry.definition;
    let access = AgentAccess::of(entry);
    PickerItem {
        section: Some(agent_section(entry.metadata.origin).1.into()),
        detail: Some(agent_detail(entry, access, models).into()),
        badge: Some(access.marker()),
        selection_verb: Some(access.selection_verb()),
        ..PickerItem::new(definition.id.to_string(), definition.id.to_string())
    }
}

fn agent_detail(
    entry: &AgentCatalogEntry,
    access: AgentAccess,
    models: &AgentModelView<'_>,
) -> DetailSheet {
    let definition = &entry.definition;
    let ModelFields {
        model,
        reasoning,
        fast,
    } = agent_model_fields(entry, models);
    let mut fields = vec![
        DetailField::new(
            "Runtime",
            definition.runtime.runtime().to_string(),
            DetailTone::Normal,
        ),
        model,
        reasoning,
        agent_tools_field(definition),
    ];
    if fast {
        fields.push(DetailField::new("Fast", "on", DetailTone::Normal));
    }
    if let AgentRuntimeSpec::ClaudeCli(config) = &definition.runtime {
        fields.push(if config.inherit_claude_config {
            DetailField::new("Claude config", "inherit", DetailTone::Normal)
        } else {
            DetailField::new("Claude config", "closed", DetailTone::Muted)
        });
    }
    fields.push(agent_source_field(entry));

    let mut blocks = vec![DetailBlock::Title {
        text: definition.id.to_string(),
        tag: Some(access.tag()),
    }];
    if !definition.description.is_empty() {
        blocks.push(DetailBlock::Paragraph(definition.description.to_string()));
    }
    blocks.push(DetailBlock::Rule);
    blocks.push(DetailBlock::Fields(fields));
    blocks.push(DetailBlock::Rule);
    blocks.extend(agent_prompt_blocks(definition, access));
    DetailSheet { blocks }
}

/// Model, reasoning, and whether the agent runs fast, as bind resolves them.
struct ModelFields {
    model: DetailField,
    reasoning: DetailField,
    fast: bool,
}

fn agent_model_fields(entry: &AgentCatalogEntry, models: &AgentModelView<'_>) -> ModelFields {
    let definition = &entry.definition;
    if entry.metadata.origin == AgentOrigin::Internal {
        return internal_agent_model_fields(entry, models);
    }
    let file_policy = definition.model_policy();
    let model_override = match crate::agent::model_override(definition, models.internal_agents) {
        Ok(model_override) => model_override,
        // Bind refuses this agent until the entry goes, so say so here.
        Err(_) => {
            let reference = models
                .internal_agents
                .get(definition.id.as_str())
                .map_or_else(String::new, InternalAgentModelConfig::display_reference);
            return ModelFields {
                model: DetailField::new("Model", reference, DetailTone::Warning)
                    .with_note("override cannot apply"),
                reasoning: reasoning_field(definition.reasoning()),
                fast: definition.fast(),
            };
        }
    };
    let (policy, reasoning) = crate::agent::effective_rho_model(
        &file_policy,
        definition.reasoning(),
        model_override.as_ref(),
    );
    let model = model_policy_field(policy);
    ModelFields {
        model: match model_override {
            Some(_) => model.with_note("override"),
            None => model,
        },
        reasoning: reasoning_field(reasoning),
        fast: policy.selection().is_some_and(|selection| selection.fast),
    }
}

fn internal_agent_model_fields(
    entry: &AgentCatalogEntry,
    models: &AgentModelView<'_>,
) -> ModelFields {
    let definition = &entry.definition;
    let id = definition.id.as_str();
    let configured = models.internal_agents.get(id);
    let model = match configured {
        Some(selection) => {
            DetailField::new("Model", selection.display_reference(), DetailTone::Normal)
                .with_note("override")
        }
        None if internal_agent_requires_model(id) => {
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
    let reasoning = match configured {
        Some(selection) => Some(effective_internal_agent_reasoning(id, selection)),
        None => definition.reasoning(),
    };
    ModelFields {
        model,
        reasoning: reasoning_field(reasoning),
        fast: definition.fast(),
    }
}

fn model_policy_field(policy: &ModelPolicy) -> DetailField {
    match policy {
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

fn reasoning_field(level: Option<crate::agent::ReasoningLevel>) -> DetailField {
    match level {
        Some(level) => DetailField::new("Reasoning", level.to_string(), DetailTone::Normal),
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

/// Prompt heading with the policy and size, then the body. Editable and
/// read-only agents show a short excerpt because Enter opens the full text
/// (editor or read-only view). Enter on an internal or model-only agent
/// configures its model instead, so the scrollable detail pane is the only
/// place its full prompt can appear, and it shows all of it.
fn agent_prompt_blocks(
    definition: &crate::agent::AgentDefinition,
    access: AgentAccess,
) -> Vec<DetailBlock> {
    let (policy, text) = prompt_policy_parts(&definition.prompt);
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
    blocks.push(match access {
        AgentAccess::Internal | AgentAccess::ModelOnly => DetailBlock::Muted(text.to_string()),
        AgentAccess::Editable | AgentAccess::ReadOnly => DetailBlock::Excerpt {
            text: text.to_string(),
            rows: PROMPT_EXCERPT_ROWS,
            anchor: ExcerptAnchor::Start,
        },
    });
    blocks
}

/// Human policy label and body of an agent prompt.
pub(super) fn prompt_policy_parts(prompt: &PromptPolicy) -> (&'static str, &str) {
    match prompt {
        PromptPolicy::Extend(text) => ("extends system prompt", text),
        PromptPolicy::Replace(text) => ("replaces system prompt", text),
    }
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

/// The picker row a stored `[internal_agents.<id>]` entry marks.
fn configured_selection(selection: &InternalAgentModelConfig) -> InternalAgentSelection {
    match &selection.target {
        InternalAgentTarget::Rho(rho) => InternalAgentSelection::RhoModel {
            provider: rho.provider.clone(),
            model: rho.model.clone(),
        },
        InternalAgentTarget::ClaudeCli { model } => InternalAgentSelection::ClaudeCode {
            model: model.clone(),
        },
    }
}

impl super::App {
    /// Builds the model picker for an agent's `[internal_agents]` entry and
    /// records what a selection means. Callers place the picker themselves,
    /// because the agents and config pickers open it as a child while
    /// `/advisor` opens it alone.
    pub(super) fn internal_agent_model_picker(&mut self, target: AgentModelTarget) -> UiPicker {
        self.refresh_available_auths();
        let id = target.id.as_str();
        let current = match &target.default {
            AgentModelDefault::Internal => self.internal_agent_picker_model(id),
            AgentModelDefault::Definition(model) => self.catalog_agent_picker_model(id, model),
        };
        let scope = self.resolved_model_picker_scope();
        let picker = super::model_picker::internal_agent_model_picker(
            super::model_picker::InternalAgentPickerInputs {
                agent_id: id,
                current: current.current,
                fallback: current.fallback,
                claude_code: claude_code_rows_for(id),
                favorite_models: &self.info.runtime.favorite_models,
                available_auths: &self.available_auths,
                scope,
                keybindings: &self.info.runtime.keybindings,
            },
        );
        self.internal_agent_model_target = Some(target);
        picker
    }

    /// Model shown as selected in an internal agent's picker, and whether the
    /// conversation-model row belongs there.
    fn internal_agent_picker_model(&self, id: &str) -> AgentPickerModel {
        let requires_model = internal_agent_requires_model(id);
        match self.info.runtime.internal_agents.get(id) {
            Some(selection) => AgentPickerModel {
                current: configured_selection(selection),
                fallback: if requires_model {
                    FallbackModelRow::Omitted
                } else {
                    FallbackModelRow::Conversation { selected: false }
                },
            },
            None if requires_model => AgentPickerModel {
                current: InternalAgentSelection::Unset,
                fallback: FallbackModelRow::Omitted,
            },
            None => AgentPickerModel {
                current: InternalAgentSelection::RhoModel {
                    provider: self.info.runtime.provider.clone(),
                    model: self.info.runtime.model.clone(),
                },
                fallback: FallbackModelRow::Conversation { selected: true },
            },
        }
    }

    /// Model shown as selected in a read-only catalog agent's picker: the
    /// override, else the model its file names, which the fallback row
    /// restores.
    fn catalog_agent_picker_model(&self, id: &str, model: &ModelPolicy) -> AgentPickerModel {
        let configured = self.info.runtime.internal_agents.get(id);
        let selected = configured.is_none();
        let (fallback, default_current) = match model.selection() {
            None => (
                FallbackModelRow::Conversation { selected },
                InternalAgentSelection::RhoModel {
                    provider: self.info.runtime.provider.clone(),
                    model: self.info.runtime.model.clone(),
                },
            ),
            Some(selection) => (
                FallbackModelRow::AgentDefault {
                    model: model_name(selection),
                    selected,
                },
                match &selection.provider {
                    Some(provider) => InternalAgentSelection::RhoModel {
                        provider: provider.clone(),
                        model: selection.model.clone(),
                    },
                    None => InternalAgentSelection::Unset,
                },
            ),
        };
        AgentPickerModel {
            current: configured.map_or(default_current, configured_selection),
            fallback,
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
        self.open_agent_model_picker(AgentModelTarget {
            id: id.to_string(),
            origin,
            default: AgentModelDefault::Internal,
        })
    }

    /// Opens the model picker for `target`, placed as
    /// [`Self::open_internal_agent_model_picker`] describes.
    fn open_agent_model_picker(&mut self, target: AgentModelTarget) -> bool {
        let origin = target.origin;
        let picker = self.internal_agent_model_picker(target);
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
        let internal = crate::agent::is_internal_agent_id(id);
        if internal {
            self.open_internal_agent_model_picker(id, InternalAgentModelPickerOrigin::AgentsPicker);
        }
        internal
    }

    /// Enter on a model-only agent in `/agents`: pick the model that
    /// overrides its file's.
    pub(super) fn open_catalog_agent_model_picker(&mut self, entry: &AgentCatalogEntry) {
        self.open_agent_model_picker(AgentModelTarget {
            id: entry.definition.id.to_string(),
            origin: InternalAgentModelPickerOrigin::AgentsPicker,
            default: AgentModelDefault::Definition(entry.definition.model_policy().into_owned()),
        });
    }

    pub(super) fn execute_agents_command(&mut self) -> anyhow::Result<()> {
        if self.open_agents_picker() {
            self.set_status("loaded agents");
        }
        Ok(())
    }

    /// Opens the agents picker on a fresh catalog. On a load failure, reports
    /// it and leaves the composer on input; returns whether the picker opened.
    pub(super) fn open_agents_picker(&mut self) -> bool {
        let Some(catalog) = self.load_agents_or_report() else {
            self.input_ui.set_composer(ComposerMode::Input);
            return false;
        };
        let picker = self.agents_picker(catalog);
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        true
    }

    /// Discovers agents for the session cwd, reporting skipped definition files
    /// and failures in the transcript. The composer is left for the caller to
    /// settle.
    pub(super) fn load_agents_or_report(&mut self) -> Option<AgentCatalog> {
        match AgentCatalog::discover(&self.info.runtime.cwd) {
            Ok(catalog) => {
                for error in catalog.skipped() {
                    self.insert_entry(&super::Entry::Error(format!(
                        "could not load agent: {error}"
                    )));
                }
                Some(catalog)
            }
            Err(error) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "could not load agents: {error}"
                )));
                self.set_status("agent load failed");
                None
            }
        }
    }

    fn agents_picker(&self, catalog: AgentCatalog) -> UiPicker {
        let mut picker = agent_picker(catalog, AgentModelView::from(&self.info.runtime));
        if let Some(target) = self.internal_agent_model_target.as_ref() {
            Self::restore_picker_position(&mut picker, &target.id, String::new());
        }
        picker
    }

    /// After `/agents` wrote or removed a definition: append any advertised
    /// catalog correction and rebuild the picker for the caller to position
    /// and open. `None` (composer on input) when the reload failed. A failed
    /// correction is reported but never undoes the disk change, which
    /// subsequent launches already use.
    pub(super) fn agents_picker_after_change(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> Option<UiPicker> {
        let Some(catalog) = self.load_agents_or_report() else {
            self.input_ui.set_composer(ComposerMode::Input);
            return None;
        };
        match agent.sync_agent_catalog(&catalog) {
            Ok(Some(display)) => self.insert_entry(&super::Entry::Notice(display.into())),
            Ok(None) => {}
            Err(error) => self.insert_entry(&super::Entry::Error(format!(
                "could not tell the model about the agent change: {error}"
            ))),
        }
        Some(self.agents_picker(catalog))
    }
}

#[cfg(test)]
#[path = "agent_picker_tests.rs"]
mod tests;
