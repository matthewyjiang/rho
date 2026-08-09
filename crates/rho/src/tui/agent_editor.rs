//! In-TUI editor for user-defined agent definitions.
//!
//! Enter on a RhoHome/Project agent opens a field editor. One [`PickerAction::EditAgent`]
//! action covers the field list, choice sub-pickers, and model picker; session phase
//! decides how the selected value is interpreted. Save serializes through the agent
//! crate helpers.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};

use super::{
    agent_picker::AgentModelView, picker_overlay::OverlayChrome, render::truncate_one_line,
    text_input::AgentField, App, ComposerMode, Entry, PickerAction, PickerBadge, PickerBadgeTone,
    PickerItem, PickerLayout, UiPicker,
};

use crate::agent::{
    AgentDefinition, AgentOrigin, AgentRuntime, AgentRuntimeSpec, ModelPolicy, PromptPolicy,
    ReasoningLevel,
};
use crate::claude_runtime::models as claude_models;
use rho_providers::model::{models_dev, ReasoningCapabilities};

/// Stable field-picker values (choice/model phases dispatch by session phase).
pub(super) const AGENT_FIELD_DESCRIPTION: &str = AgentField::Description.value();
pub(super) const AGENT_FIELD_PROMPT_POLICY: &str = "agent_field:prompt_policy";
pub(super) const AGENT_FIELD_PROMPT_BODY: &str = "agent_field:prompt_body";
pub(super) const AGENT_FIELD_RUNTIME: &str = "agent_field:runtime";
pub(super) const AGENT_FIELD_MODEL_POLICY: &str = "agent_field:model_policy";
pub(super) const AGENT_FIELD_MODEL: &str = AgentField::Model.value();
pub(super) const AGENT_FIELD_PROVIDER: &str = AgentField::Provider.value();
pub(super) const AGENT_FIELD_AUTH: &str = "agent_field:auth";
pub(super) const AGENT_FIELD_REASONING: &str = "agent_field:reasoning";
pub(super) const AGENT_FIELD_TOOLS: &str = AgentField::Tools.value();
pub(super) const AGENT_FIELD_INHERIT_CLAUDE_CONFIG: &str = "agent_field:inherit_claude_config";
pub(super) const AGENT_FIELD_SAVE: &str = "agent_field:save";
pub(super) const AGENT_FIELD_CANCEL: &str = "agent_field:cancel";

/// How the active EditAgent picker interprets Enter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentEditPhase {
    Fields,
    Choosing(AgentChoiceField),
    PickingModel,
}

/// Choice sub-picker fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentChoiceField {
    PromptPolicy,
    Runtime,
    ModelPolicy,
    /// Claude `--model`, chosen from the offered aliases rather than typed.
    ClaudeModel,
    Auth,
    Reasoning,
    InheritClaudeConfig,
}

impl AgentChoiceField {
    const fn field_value(self) -> &'static str {
        match self {
            Self::PromptPolicy => AGENT_FIELD_PROMPT_POLICY,
            Self::Runtime => AGENT_FIELD_RUNTIME,
            Self::ModelPolicy => AGENT_FIELD_MODEL_POLICY,
            Self::ClaudeModel => AGENT_FIELD_MODEL,
            Self::Auth => AGENT_FIELD_AUTH,
            Self::Reasoning => AGENT_FIELD_REASONING,
            Self::InheritClaudeConfig => AGENT_FIELD_INHERIT_CLAUDE_CONFIG,
        }
    }

    const fn choice_prefix(self) -> &'static str {
        match self {
            Self::PromptPolicy => "agent_choice:prompt_policy:",
            Self::Runtime => "agent_choice:runtime:",
            Self::ModelPolicy => "agent_choice:model_policy:",
            Self::ClaudeModel => "agent_choice:claude_model:",
            Self::Auth => "agent_choice:auth:",
            Self::Reasoning => "agent_choice:reasoning:",
            Self::InheritClaudeConfig => "agent_choice:inherit_claude_config:",
        }
    }

    pub(super) const fn status_label(self) -> &'static str {
        match self {
            Self::PromptPolicy => "prompt policy",
            Self::Runtime => "runtime",
            Self::ModelPolicy => "model policy",
            Self::ClaudeModel => "claude model",
            Self::Auth => "auth",
            Self::Reasoning => "reasoning",
            Self::InheritClaudeConfig => "inherit Claude config",
        }
    }
}

/// One authorized edit session with reversible runtime stash.
pub(super) struct AgentEditSession {
    draft: AgentDefinition,
    path: PathBuf,
    origin: AgentOrigin,
    authorized_root: PathBuf,
    original_contents: String,
    phase: AgentEditPhase,
    rho_runtime: Option<AgentRuntimeSpec>,
    claude_runtime: Option<AgentRuntimeSpec>,
}

impl AgentEditSession {
    fn new(
        draft: AgentDefinition,
        path: PathBuf,
        origin: AgentOrigin,
        authorized_root: PathBuf,
        original_contents: String,
    ) -> Self {
        let (rho_runtime, claude_runtime) = match &draft.runtime {
            runtime @ AgentRuntimeSpec::Rho { .. } => (Some(runtime.clone()), None),
            runtime @ AgentRuntimeSpec::ClaudeCli(_) => (None, Some(runtime.clone())),
        };
        Self {
            draft,
            path,
            origin,
            authorized_root,
            original_contents,
            phase: AgentEditPhase::Fields,
            rho_runtime,
            claude_runtime,
        }
    }

    pub(super) fn draft(&self) -> &AgentDefinition {
        &self.draft
    }

    pub(super) fn phase(&self) -> AgentEditPhase {
        self.phase
    }

    pub(super) fn set_phase(&mut self, phase: AgentEditPhase) {
        self.phase = phase;
    }

    pub(super) fn with_draft_mut<R>(&mut self, f: impl FnOnce(&mut AgentDefinition) -> R) -> R {
        f(&mut self.draft)
    }

    fn switch_runtime(&mut self, value: &str) -> bool {
        match &self.draft.runtime {
            runtime @ AgentRuntimeSpec::Rho { .. } => self.rho_runtime = Some(runtime.clone()),
            runtime @ AgentRuntimeSpec::ClaudeCli(_) => {
                self.claude_runtime = Some(runtime.clone());
            }
        }
        let saved = match value {
            "rho" => self.rho_runtime.clone(),
            "claude-cli" => self.claude_runtime.clone(),
            _ => return false,
        };
        if let Some(runtime) = saved {
            self.draft.runtime = runtime;
            true
        } else {
            self.draft.switch_runtime_kind(value)
        }
    }
}

/// Returns the authorized source root and rejects paths that could escape it.
fn authorize_editable_path(
    origin: AgentOrigin,
    path: &Path,
    cwd: &Path,
) -> anyhow::Result<PathBuf> {
    let roots = match origin {
        AgentOrigin::RhoHome => crate::paths::home_dir()
            .map(|home| {
                let root = home.join(".rho/agents");
                vec![(home, root)]
            })
            .unwrap_or_default(),
        AgentOrigin::Project => crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .map(|base| {
                let root = base.join(".agents/agents");
                (base, root)
            })
            .collect(),
        _ => Vec::new(),
    };
    let (base, root) = roots
        .into_iter()
        .find(|(_, root)| path.parent() == Some(root.as_path()))
        .ok_or_else(|| anyhow!("agent source is outside an editable agent directory"))?;
    let mut component_path = base;
    for component in root
        .strip_prefix(&component_path)
        .context("editable agent root is outside its source base")?
        .components()
    {
        component_path.push(component);
        let metadata = fs::symlink_metadata(&component_path)
            .with_context(|| format!("could not inspect {}", component_path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(anyhow!(
                "editable agent directory must not contain symlinks"
            ));
        }
    }
    let root_metadata = fs::symlink_metadata(&root)
        .with_context(|| format!("could not inspect {}", root.display()))?;
    let file_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(anyhow!("editable agent directory must not be a symlink"));
    }
    if file_metadata.file_type().is_symlink() || !file_metadata.is_file() {
        return Err(anyhow!("editable agent source must be a regular file"));
    }
    Ok(root)
}

fn badge(text: impl Into<String>) -> PickerBadge {
    PickerBadge {
        text: text.into(),
        tone: PickerBadgeTone::Selected,
    }
}

fn field_item(
    label: &str,
    detail: impl Into<String>,
    badge_text: Option<String>,
    value: &'static str,
) -> PickerItem {
    PickerItem {
        section: None,
        label: label.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: badge_text.map(badge),
        value: value.into(),
        selection_verb: None,
    }
}

/// Builds the agent field editor picker for `draft`.
pub(super) fn agent_field_picker(draft: &AgentDefinition) -> UiPicker {
    let runtime = draft.runtime.runtime();
    let model_policy = draft.model_policy();

    let mut items = vec![
        field_item(
            "Description",
            "One-line summary shown in the agents picker. At most 1024 characters.",
            Some(if draft.description.is_empty() {
                "unset".into()
            } else {
                truncate_one_line(&draft.description, 48)
            }),
            AGENT_FIELD_DESCRIPTION,
        ),
        field_item(
            "Prompt policy",
            "Extend adds the body to the system prompt; replace uses the body as the full prompt.",
            Some(
                match &draft.prompt {
                    PromptPolicy::Extend(_) => "extend",
                    PromptPolicy::Replace(_) => "replace",
                }
                .into(),
            ),
            AGENT_FIELD_PROMPT_POLICY,
        ),
        field_item(
            "Prompt body",
            format!(
                "Edit the prompt body in $EDITOR.\n\nCurrent body\n{}",
                prompt_body_preview(draft)
            ),
            None,
            AGENT_FIELD_PROMPT_BODY,
        ),
        field_item(
            "Runtime",
            "Which harness runs this agent. Switching resets incompatible fields.",
            Some(runtime.to_string()),
            AGENT_FIELD_RUNTIME,
        ),
    ];

    match runtime {
        AgentRuntime::Rho => {
            items.push(field_item(
                "Model policy",
                "inherit uses the conversation model; prefer/require/select pins a model.",
                Some(draft.model_policy_badge()),
                AGENT_FIELD_MODEL_POLICY,
            ));
            if !matches!(model_policy.as_ref(), ModelPolicy::Inherit) {
                items.push(field_item(
                    "Model",
                    "Model name for the selected policy. Edit as text; validated at save.",
                    Some(draft.model_badge()),
                    AGENT_FIELD_MODEL,
                ));
                let provider = match model_policy.as_ref() {
                    ModelPolicy::Prefer(selection)
                    | ModelPolicy::Require(selection)
                    | ModelPolicy::Select(selection) => {
                        selection.provider.clone().unwrap_or_else(|| "auto".into())
                    }
                    ModelPolicy::Inherit => "auto".into(),
                };
                items.push(field_item(
                    "Provider",
                    "Optional provider for the selected model. Leave empty to let Rho resolve it.",
                    Some(provider),
                    AGENT_FIELD_PROVIDER,
                ));
                items.push(field_item(
                    "Auth",
                    "Auth profile for the pinned provider. Host keeps a compatible login when unset.",
                    Some(draft.auth_badge()),
                    AGENT_FIELD_AUTH,
                ));
            }
            items.push(field_item(
                "Reasoning",
                "Reasoning level for this agent. Omit to inherit the conversation setting.",
                draft.reasoning().map(|level| level.to_string()),
                AGENT_FIELD_REASONING,
            ));
            items.push(field_item(
                "Tools",
                "Rho tool capabilities, as a bracket list (for example [read_file, shell]) or all.",
                Some(draft.tools_badge()),
                AGENT_FIELD_TOOLS,
            ));
        }
        AgentRuntime::ClaudeCli => {
            items.push(field_item(
                "Model",
                "Claude model alias passed as --model. Default lets Claude Code choose.",
                Some(claude_model_badge(draft)),
                AGENT_FIELD_MODEL,
            ));
            items.push(field_item(
                "Reasoning",
                "Claude --effort level. Claude does not accept off or minimal.",
                draft.reasoning().map(|level| level.to_string()),
                AGENT_FIELD_REASONING,
            ));
            items.push(field_item(
                "Tools",
                "Claude tool names, as a bracket list (for example [Read, Edit, \"Bash(git *)\"]).",
                Some(draft.tools_badge()),
                AGENT_FIELD_TOOLS,
            ));
            let inherit = matches!(
                &draft.runtime,
                AgentRuntimeSpec::ClaudeCli(config) if config.inherit_claude_config
            );
            items.push(field_item(
                "Inherit Claude config",
                "When yes, widen Claude setting sources to the user's full Claude config.",
                Some(if inherit { "yes" } else { "no" }.into()),
                AGENT_FIELD_INHERIT_CLAUDE_CONFIG,
            ));
        }
    }

    items.push(field_item(
        "Save",
        "Serialize, validate by re-parsing, and write the agent file.",
        None,
        AGENT_FIELD_SAVE,
    ));
    items.push(field_item(
        "Cancel",
        "Discard edits and return to the agents picker.",
        None,
        AGENT_FIELD_CANCEL,
    ));

    UiPicker::new(
        format!("edit agent {}", draft.id),
        items,
        PickerAction::EditAgent,
    )
    .with_layout(PickerLayout::Overlay)
    .with_confirm_verb("edit")
    .with_overlay_chrome(OverlayChrome {
        nav_label: " EDIT AGENT".into(),
        detail_label: Some(" DETAILS".into()),
        nav_keys_hint: "↑↓ fields".into(),
    })
}

/// Badge for the Claude model row. Unset reads as the Claude Code default
/// rather than Rho's `inherit`, which names a different concept.
fn claude_model_badge(draft: &AgentDefinition) -> String {
    let model = draft.model_text();
    if model.is_empty() {
        claude_models::CLAUDE_DEFAULT_MODEL_BADGE.into()
    } else {
        model
    }
}

/// Rows for the Claude `--model` choice: the default, every offered alias, and
/// the configured value when it is neither. Claude accepts full model ids that
/// Rho does not offer, so a hand-written definition keeps its model and stays
/// editable instead of being silently rewritten.
fn claude_model_choice_items(draft: &AgentDefinition, prefix: &str) -> Vec<PickerItem> {
    let current = draft.model_text();
    let mut items = vec![PickerItem {
        section: None,
        label: claude_models::CLAUDE_DEFAULT_MODEL_LABEL.into(),
        detail: Some(claude_models::CLAUDE_DEFAULT_MODEL_DETAIL.into()),
        preview: None,
        badge: current.is_empty().then(|| badge("selected")),
        value: prefix.to_string(),
        selection_verb: None,
    }];
    items.extend(
        claude_models::CLAUDE_MODEL_ALIASES
            .iter()
            .map(|alias| PickerItem {
                section: None,
                label: alias.name.into(),
                detail: Some(alias.detail.into()),
                preview: None,
                badge: (current == alias.name).then(|| badge("selected")),
                value: format!("{prefix}{}", alias.name),
                selection_verb: None,
            }),
    );
    if !current.is_empty() && !claude_models::is_offered_alias(&current) {
        items.push(PickerItem {
            section: None,
            label: current.clone(),
            detail: Some("Set in the agent file. Passed through as --model unchanged.".into()),
            preview: None,
            badge: Some(badge("selected")),
            value: format!("{prefix}{current}"),
            selection_verb: None,
        });
    }
    items
}

fn prompt_body_preview(draft: &AgentDefinition) -> String {
    let body = match &draft.prompt {
        PromptPolicy::Extend(text) | PromptPolicy::Replace(text) => text.as_str(),
    };
    if body.is_empty() {
        "(empty)".into()
    } else {
        truncate_one_line(body, 80)
    }
}

fn agent_choice_picker(field: AgentChoiceField, draft: &AgentDefinition) -> UiPicker {
    debug_assert!(
        !matches!(field, AgentChoiceField::Auth),
        "use auth_choice_picker for auth"
    );
    let prefix = field.choice_prefix();
    let (title, items) = match field {
        AgentChoiceField::PromptPolicy => {
            let current = match &draft.prompt {
                PromptPolicy::Extend(_) => "extend",
                PromptPolicy::Replace(_) => "replace",
            };
            (
                "prompt policy",
                choice_items(
                    &[
                        ("extend", "Add the body to the system prompt."),
                        (
                            "replace",
                            "Use the body as the full system prompt. Must be non-empty.",
                        ),
                    ],
                    current,
                    prefix,
                ),
            )
        }
        AgentChoiceField::Runtime => {
            let current = draft.runtime.runtime().as_str();
            (
                "runtime",
                choice_items(
                    &[
                        ("rho", "Rho's own loop and tool vocabulary."),
                        ("claude-cli", "Delegate the loop to the claude binary."),
                    ],
                    current,
                    prefix,
                ),
            )
        }
        AgentChoiceField::ModelPolicy => {
            let is_claude = draft.runtime.runtime() == AgentRuntime::ClaudeCli;
            let current = draft.model_policy_badge();
            let options: &[(&str, &str)] = if is_claude {
                &[
                    ("inherit", "Inherit Claude's default model."),
                    ("select", "Pin a Claude model name as --model."),
                ]
            } else {
                &[
                    ("inherit", "Use the conversation model."),
                    ("prefer", "Prefer a model, falling back if unavailable."),
                    ("require", "Require a model, failing if unavailable."),
                    ("select", "Pin a model."),
                ]
            };
            ("model policy", choice_items(options, &current, prefix))
        }
        AgentChoiceField::ClaudeModel => ("claude model", claude_model_choice_items(draft, prefix)),
        AgentChoiceField::Auth => unreachable!("use auth_choice_picker"),
        AgentChoiceField::Reasoning => {
            let is_claude = draft.runtime.runtime() == AgentRuntime::ClaudeCli;
            let current = draft.reasoning();
            let levels = selectable_agent_reasoning_levels(draft);
            let mut items = vec![PickerItem {
                section: None,
                label: "inherit".into(),
                detail: Some("Omit reasoning; inherit the conversation setting.".into()),
                preview: None,
                badge: current.is_none().then(|| badge("selected")),
                value: format!("{prefix}inherit"),
                selection_verb: None,
            }];
            items.extend(levels.into_iter().map(|level| {
                let selected = current == Some(level);
                PickerItem {
                    section: None,
                    label: level.to_string(),
                    detail: Some(
                        if is_claude {
                            "Claude --effort level."
                        } else {
                            "Reasoning level for this agent."
                        }
                        .into(),
                    ),
                    preview: None,
                    badge: selected.then(|| badge("selected")),
                    value: format!("{prefix}{level}"),
                    selection_verb: None,
                }
            }));
            ("reasoning", items)
        }
        AgentChoiceField::InheritClaudeConfig => {
            let current = match &draft.runtime {
                AgentRuntimeSpec::ClaudeCli(config) if config.inherit_claude_config => "yes",
                _ => "no",
            };
            (
                "inherit Claude config",
                choice_items(
                    &[
                        ("no", "Closed: only frontmatter settings."),
                        ("yes", "Widen to the user's full Claude config."),
                    ],
                    current,
                    prefix,
                ),
            )
        }
    };
    UiPicker::new(title, items, PickerAction::EditAgent).with_confirm_verb("set")
}

fn auth_choice_picker(draft: &AgentDefinition, available_auths: &[String]) -> UiPicker {
    let prefix = AgentChoiceField::Auth.choice_prefix();
    let current = draft.auth_text();
    let provider = draft.provider_text();
    let mut items = vec![PickerItem {
        section: None,
        label: "host".into(),
        detail: Some(
            "Do not pin auth. Keep the conversation login when it fits the provider.".into(),
        ),
        preview: None,
        badge: current.is_empty().then(|| badge("selected")),
        // Empty id means unset pin (display label remains "host").
        value: prefix.to_string(),
        selection_verb: None,
    }];
    let mut modes: Vec<(String, String)> = available_auths
        .iter()
        .filter_map(|auth| {
            let (_descriptor, mode) = rho_providers::provider::resolve_auth_mode(auth)?;
            if !provider.is_empty()
                && !rho_providers::provider::provider_accepts_auth(&provider, mode.id)
            {
                return None;
            }
            Some((mode.id.to_string(), mode.login_label.to_string()))
        })
        .collect();
    // Keep a configured but unavailable auth visible so it can be cleared.
    if !current.is_empty() && !modes.iter().any(|(id, _)| id == &current) {
        let label = rho_providers::provider::resolve_auth_mode(&current)
            .map(|(_, mode)| mode.login_label.to_string())
            .unwrap_or_else(|| current.clone());
        modes.push((current.clone(), format!("{label} (not available)")));
    }
    modes.sort_by(|left, right| left.1.cmp(&right.1));
    items.extend(modes.into_iter().map(|(id, label)| {
        let selected = id == current;
        PickerItem {
            section: None,
            label,
            detail: Some(format!("Pin auth profile {id}.")),
            preview: None,
            badge: selected.then(|| badge("selected")),
            value: format!("{prefix}{id}"),
            selection_verb: None,
        }
    }));
    UiPicker::new("auth", items, PickerAction::EditAgent).with_confirm_verb("set")
}

fn choice_items(options: &[(&str, &str)], current: &str, value_prefix: &str) -> Vec<PickerItem> {
    options
        .iter()
        .map(|(label, detail)| {
            let selected = *label == current;
            PickerItem {
                section: None,
                label: (*label).into(),
                detail: Some((*detail).into()),
                preview: None,
                badge: selected.then(|| badge("selected")),
                value: format!("{value_prefix}{label}"),
                selection_verb: None,
            }
        })
        .collect()
}

/// Reasoning options for the agent editor.
///
/// Prefer catalog-advertised levels when the draft pins a provider/model.
/// Fall back to the full (or Claude-safe) set when catalog data is missing.
fn selectable_agent_reasoning_levels(draft: &AgentDefinition) -> Vec<ReasoningLevel> {
    let is_claude = draft.runtime.runtime() == AgentRuntime::ClaudeCli;
    // Claude Code efforts are fixed; Claude model ids are not resolved through
    // models.dev provider rows in this editor.
    let capabilities = if is_claude {
        ReasoningCapabilities::Unknown
    } else {
        draft_model_reasoning_capabilities(draft)
    };
    let fallback = if is_claude {
        crate::claude_runtime::spawn::CLAUDE_EFFORT_LEVELS.levels()
    } else {
        ReasoningLevel::ALL.as_slice()
    };
    capabilities.selectable_levels(fallback, draft.reasoning())
}

fn draft_model_reasoning_capabilities(draft: &AgentDefinition) -> ReasoningCapabilities {
    let model_policy = draft.model_policy();
    let Some(selection) = model_policy.selection() else {
        return ReasoningCapabilities::Unknown;
    };
    if selection.model.is_empty() {
        return ReasoningCapabilities::Unknown;
    }
    let Some(provider) = selection.provider.as_deref() else {
        return ReasoningCapabilities::Unknown;
    };
    models_dev::known_reasoning_capabilities(provider, &selection.model)
}

#[path = "agent_editor_app.rs"]
mod app;

#[cfg(test)]
#[path = "agent_editor_tests.rs"]
mod tests;
