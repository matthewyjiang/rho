//! App state transitions for the user-defined agent editor.

use std::fs;

use ratatui::DefaultTerminal;

use super::*;
use crate::agent::{
    save_definition, AgentCatalog, AgentRuntime, PromptPolicy, SaveDefinitionError,
};
use crate::tui::text_input::{AgentField, TextInput};

impl App {
    /// Handles Enter on an agent in the `/agents` picker.
    pub(in crate::tui) fn submit_view_agent_selection(
        &mut self,
        value: &str,
    ) -> anyhow::Result<()> {
        if self.open_selected_internal_agent_model_picker(value) {
            return Ok(());
        }
        let catalog = match AgentCatalog::discover(&self.info.runtime.cwd) {
            Ok(catalog) => catalog,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not load agents: {error}")));
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("agent load failed");
                return Ok(());
            }
        };
        let entry = match catalog.find(value) {
            Ok(entry) => entry,
            Err(error) => {
                self.insert_entry(&Entry::Error(error.to_string()));
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("agent load failed");
                return Ok(());
            }
        };
        let editable = matches!(
            entry.metadata.origin,
            AgentOrigin::RhoHome | AgentOrigin::Project
        ) && entry.metadata.path.is_some();
        if !editable {
            self.agent_editor_session = None;
            self.input_ui.set_composer(ComposerMode::Input);
            self.set_status("ready");
            return Ok(());
        }

        let path = entry
            .metadata
            .path
            .clone()
            .expect("editable origin has a path");
        let authorized_root =
            match authorize_editable_path(entry.metadata.origin, &path, &self.info.runtime.cwd) {
                Ok(root) => root,
                Err(error) => {
                    self.insert_entry(&Entry::Error(format!(
                        "agent source is not safe to edit: {error}"
                    )));
                    self.set_status("agent is read-only");
                    return Ok(());
                }
            };
        let original_contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not read agent source: {error}"
                )));
                self.set_status("agent load failed");
                return Ok(());
            }
        };
        let draft = match crate::agent::parse_definition(&path, value, &original_contents) {
            Ok(draft) => draft,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "agent changed while opening it: {error}"
                )));
                self.set_status("agent load failed");
                return Ok(());
            }
        };
        let picker = agent_field_picker(&draft);
        self.agent_editor_session = Some(AgentEditSession::new(
            draft.clone(),
            path,
            entry.metadata.origin,
            authorized_root,
            original_contents,
        ));
        self.open_child_picker(picker);
        self.set_status(format!("edit agent {}", draft.id));
        Ok(())
    }

    /// Handles Enter for any EditAgent picker (fields, choices, or model).
    pub(in crate::tui) async fn submit_edit_agent_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let phase = self
            .agent_editor_session
            .as_ref()
            .map(AgentEditSession::phase);
        let Some(phase) = phase else {
            self.cancel_agent_editor();
            return Ok(());
        };
        match phase {
            AgentEditPhase::Fields => self.submit_agent_field_selection(value, terminal).await,
            AgentEditPhase::Choosing(field) => {
                self.submit_agent_field_choice(field, value);
                Ok(())
            }
            AgentEditPhase::PickingModel => {
                self.submit_agent_model_selection(value);
                Ok(())
            }
        }
    }

    async fn submit_agent_field_selection(
        &mut self,
        value: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(draft) = self
            .agent_editor_session
            .as_ref()
            .map(|session| session.draft().clone())
        else {
            self.cancel_agent_editor();
            return Ok(());
        };
        match value {
            AGENT_FIELD_DESCRIPTION => {
                self.open_agent_text_input(AgentField::Description, draft.description.clone());
            }
            AGENT_FIELD_PROMPT_POLICY => {
                self.open_agent_choice(AgentChoiceField::PromptPolicy, &draft);
            }
            AGENT_FIELD_PROMPT_BODY => {
                self.open_agent_prompt_body_editor(terminal).await?;
            }
            AGENT_FIELD_RUNTIME => {
                self.open_agent_choice(AgentChoiceField::Runtime, &draft);
            }
            AGENT_FIELD_MODEL_POLICY => {
                self.open_agent_choice(AgentChoiceField::ModelPolicy, &draft);
            }
            AGENT_FIELD_MODEL => self.open_agent_model_or_text(&draft),
            AGENT_FIELD_PROVIDER => {
                self.open_agent_text_input(AgentField::Provider, draft.provider_text());
            }
            AGENT_FIELD_AUTH => {
                self.open_agent_choice(AgentChoiceField::Auth, &draft);
            }
            AGENT_FIELD_REASONING => {
                self.open_agent_choice(AgentChoiceField::Reasoning, &draft);
            }
            AGENT_FIELD_TOOLS => {
                self.open_agent_text_input(AgentField::Tools, draft.tools_text());
            }
            AGENT_FIELD_INHERIT_CLAUDE_CONFIG => {
                self.open_agent_choice(AgentChoiceField::InheritClaudeConfig, &draft);
            }
            AGENT_FIELD_SAVE => self.save_agent_editor()?,
            AGENT_FIELD_CANCEL => self.cancel_agent_editor(),
            _ => {}
        }
        Ok(())
    }

    fn open_agent_choice(
        &mut self,
        field: AgentChoiceField,
        draft: &crate::agent::AgentDefinition,
    ) {
        if let Some(session) = &mut self.agent_editor_session {
            session.set_phase(AgentEditPhase::Choosing(field));
        }
        let picker = if matches!(field, AgentChoiceField::Auth) {
            self.refresh_available_auths();
            auth_choice_picker(draft, &self.available_auths)
        } else {
            agent_choice_picker(field, draft)
        };
        self.open_child_picker(picker);
        self.set_status(match field {
            AgentChoiceField::PromptPolicy => "prompt policy",
            AgentChoiceField::Runtime => "runtime",
            AgentChoiceField::ModelPolicy => "model policy",
            AgentChoiceField::Auth => "auth",
            AgentChoiceField::Reasoning => "reasoning",
            AgentChoiceField::InheritClaudeConfig => "inherit Claude config",
        });
    }

    fn submit_agent_field_choice(&mut self, field: AgentChoiceField, value: &str) {
        let Some(rest) = value.strip_prefix(field.choice_prefix()) else {
            self.cancel_agent_editor();
            return;
        };
        let applied = match field {
            AgentChoiceField::Runtime => self
                .agent_editor_session
                .as_mut()
                .is_some_and(|session| session.switch_runtime(rest)),
            other => self
                .agent_editor_session
                .as_mut()
                .map(|session| {
                    session.with_draft_mut(|draft| match other {
                        AgentChoiceField::PromptPolicy => draft.set_prompt_policy_kind(rest),
                        AgentChoiceField::ModelPolicy => draft.set_model_policy_kind(rest),
                        AgentChoiceField::Auth => {
                            if rest.is_empty() {
                                draft.set_auth_selection(None)
                            } else {
                                draft.set_auth_selection(Some(rest.to_string()))
                            }
                        }
                        AgentChoiceField::Reasoning => draft.set_reasoning_kind(rest),
                        AgentChoiceField::InheritClaudeConfig => {
                            draft.set_inherit_claude_config(rest)
                        }
                        AgentChoiceField::Runtime => unreachable!("handled above"),
                    })
                })
                .unwrap_or(false),
        };
        if !applied {
            self.insert_entry(&Entry::Error(format!(
                "could not apply {value} to the agent draft"
            )));
            return;
        }
        self.reopen_agent_field_picker(field.field_value());
    }

    fn submit_agent_model_selection(&mut self, value: &str) {
        let Some(mut draft) = self
            .agent_editor_session
            .as_ref()
            .map(|session| session.draft().clone())
        else {
            self.cancel_agent_editor();
            return;
        };
        self.refresh_available_auths();
        let current = draft.current_selection();
        let current_provider = current
            .provider
            .as_deref()
            .unwrap_or(&self.info.runtime.provider);
        let current_auth = self.info.runtime.auth.clone();
        match self.resolve_model_selection(value, current_provider, &current_auth) {
            Ok(resolved) => {
                let catalog = &resolved.selection;
                let mut selection = current;
                selection.provider = Some(catalog.provider.clone());
                selection.model = catalog.model.clone();
                // Model picker resolves runtime auth; only keep an existing agent
                // pin when it still belongs on the chosen provider.
                selection.auth = selection.auth.filter(|auth| {
                    rho_providers::provider::provider_accepts_auth(&catalog.provider, auth)
                });
                draft.set_model_selection(Some(selection));
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.reopen_agent_field_picker(AGENT_FIELD_MODEL);
                self.set_status("agent model switch failed");
                return;
            }
        }
        if let Some(session) = &mut self.agent_editor_session {
            session.with_draft_mut(|session_draft| *session_draft = draft);
        }
        self.reopen_agent_field_picker(AGENT_FIELD_MODEL);
    }

    fn open_agent_model_or_text(&mut self, draft: &crate::agent::AgentDefinition) {
        if draft.runtime.runtime() == AgentRuntime::ClaudeCli {
            self.open_agent_text_input(AgentField::Model, draft.model_text());
            return;
        }
        self.refresh_available_auths();
        let mut picker =
            crate::tui::model_picker::model_picker(&self.info.runtime, &self.available_auths);
        if picker.items.is_empty() {
            self.open_agent_text_input(AgentField::Model, draft.model_text());
            return;
        }
        picker.action = PickerAction::EditAgent;
        picker = picker.with_fuzzy_filter();
        if let Some(session) = &mut self.agent_editor_session {
            session.set_phase(AgentEditPhase::PickingModel);
        }
        self.open_child_picker(picker);
        self.set_status("select model");
    }

    fn open_agent_text_input(&mut self, field: AgentField, value: String) {
        let return_picker = match self.input_ui.take_composer() {
            ComposerMode::Picker(picker) => Some(picker),
            composer => {
                self.input_ui.set_composer(composer);
                None
            }
        };
        let mut input = TextInput::agent_field(field, value);
        if let Some(picker) = return_picker {
            input = input.with_return_picker(picker);
        }
        self.input_ui.set_composer(ComposerMode::TextInput(input));
        self.set_status(format!("edit {}", field.label()));
    }

    pub(in crate::tui) fn reopen_agent_field_picker(&mut self, selected_value: &str) {
        let (filter, parent) = match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) if picker.action == PickerAction::EditAgent => {
                match picker.take_parent() {
                    Some(mut field_picker) => {
                        (field_picker.filter.clone(), field_picker.take_parent())
                    }
                    None => (String::new(), None),
                }
            }
            ComposerMode::Picker(picker) => (picker.filter.clone(), picker.take_parent()),
            ComposerMode::TextInput(input) => match input.take_return_picker() {
                Some(mut picker) => (picker.filter.clone(), picker.take_parent()),
                None => (String::new(), None),
            },
            _ => (String::new(), None),
        };
        let Some(draft) = self
            .agent_editor_session
            .as_ref()
            .map(|session| session.draft().clone())
        else {
            self.cancel_agent_editor();
            return;
        };
        if let Some(session) = &mut self.agent_editor_session {
            session.set_phase(AgentEditPhase::Fields);
        }
        let mut picker = agent_field_picker(&draft);
        Self::restore_picker_position(&mut picker, selected_value, filter);
        if let Some(parent) = parent {
            picker = picker.with_parent(parent);
        }
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        self.set_status(format!("edit agent {}", draft.id));
    }

    fn save_agent_editor(&mut self) -> anyhow::Result<()> {
        let Some(session) = &self.agent_editor_session else {
            self.cancel_agent_editor();
            return Ok(());
        };
        let draft = session.draft().clone();
        let path = session.path.clone();
        let authorized_root = session.authorized_root.clone();
        let origin = session.origin;
        let original_contents = session.original_contents.clone();
        if let Some(message) = draft.validate_for_edit() {
            self.insert_entry(&Entry::Error(message));
            self.reopen_agent_field_picker(AGENT_FIELD_SAVE);
            self.set_status("agent validation failed");
            return Ok(());
        }
        let current_root = authorize_editable_path(origin, &path, &self.info.runtime.cwd);
        if !matches!(current_root, Ok(ref root) if root == &authorized_root) {
            self.insert_entry(&Entry::Error(
                "agent source is no longer safe to edit; save cancelled".into(),
            ));
            self.reopen_agent_field_picker(AGENT_FIELD_SAVE);
            self.set_status("agent save failed");
            return Ok(());
        }
        match save_definition(&draft, &path, &original_contents) {
            Ok(_contents) => {
                let id = draft.id.to_string();
                self.agent_editor_session = None;
                let catalog = match AgentCatalog::discover(&self.info.runtime.cwd) {
                    Ok(catalog) => catalog,
                    Err(error) => {
                        self.insert_entry(&Entry::Error(format!(
                            "agent saved, but could not reload agents: {error}"
                        )));
                        self.input_ui.set_composer(ComposerMode::Input);
                        self.set_status("agent reload failed");
                        return Ok(());
                    }
                };
                let mut picker = crate::tui::agent_picker::agent_picker(
                    catalog,
                    AgentModelView::from(&self.info.runtime),
                );
                Self::restore_picker_position(&mut picker, &id, String::new());
                self.input_ui.set_composer(ComposerMode::Picker(picker));
                self.set_status(format!("agent {id} saved"));
            }
            Err(SaveDefinitionError::Validation(message)) => {
                self.insert_entry(&Entry::Error(format!("agent validation failed: {message}")));
                self.reopen_agent_field_picker(AGENT_FIELD_SAVE);
                self.set_status("agent validation failed");
            }
            Err(SaveDefinitionError::Conflict) => {
                self.insert_entry(&Entry::Error(
                    "agent file changed since editing began; reload it before saving".into(),
                ));
                self.reopen_agent_field_picker(AGENT_FIELD_SAVE);
                self.set_status("agent save conflict");
            }
            Err(SaveDefinitionError::Write(message)) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not write agent file: {message}"
                )));
                self.reopen_agent_field_picker(AGENT_FIELD_SAVE);
                self.set_status("agent save failed");
            }
        }
        Ok(())
    }

    pub(in crate::tui) fn cancel_agent_editor(&mut self) {
        self.agent_editor_session = None;
        self.input_ui.set_composer(ComposerMode::Input);
        let _ = self.execute_agents_command();
        if self.status() != "agent reload failed" {
            self.set_status("agent edit cancelled");
        }
    }

    pub(super) async fn open_agent_prompt_body_editor(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(draft) = self
            .agent_editor_session
            .as_ref()
            .map(|session| session.draft().clone())
        else {
            self.cancel_agent_editor();
            return Ok(());
        };
        let body = match &draft.prompt {
            PromptPolicy::Extend(text) | PromptPolicy::Replace(text) => text.clone(),
        };
        match crate::tui::external_editor::edit_buffer_in_external_editor(
            self,
            terminal,
            &body,
            "prompt body",
        )
        .await?
        {
            Some(text) => {
                if let Some(session) = &mut self.agent_editor_session {
                    session.with_draft_mut(|draft| draft.set_prompt_body(text));
                }
                self.reopen_agent_field_picker(AGENT_FIELD_PROMPT_BODY);
                self.set_status("prompt body updated");
            }
            None => {
                self.reopen_agent_field_picker(AGENT_FIELD_PROMPT_BODY);
            }
        }
        Ok(())
    }

    /// Commits or cancels the shared text input when it targets an agent field.
    pub(in crate::tui) fn commit_agent_text_input(
        &mut self,
        field: AgentField,
        value: String,
    ) -> anyhow::Result<()> {
        let Some(session) = self.agent_editor_session.as_mut() else {
            self.cancel_agent_editor();
            return Ok(());
        };
        let result = session.with_draft_mut(|draft| match field {
            AgentField::Description => {
                draft.set_description_text(value);
                Ok(())
            }
            AgentField::Model => {
                draft.set_model_text(value);
                Ok(())
            }
            AgentField::Provider => {
                draft.set_provider_text(value);
                Ok(())
            }
            AgentField::Tools => draft.set_tools_text(&value),
        });
        match result {
            Ok(()) => {
                self.reopen_agent_field_picker(field.value());
            }
            Err(message) => {
                self.insert_entry(&Entry::Error(format!("tools: {message}")));
                self.reopen_agent_field_picker(field.value());
                self.set_status("tools edit failed");
            }
        }
        Ok(())
    }
}
