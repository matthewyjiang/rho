//! Delete a user-defined agent from the `/agents` picker.
//!
//! Delete on an editable agent (`~/.rho/agents` or a trusted project
//! `.agents/agents`) asks for confirmation, then removes the definition file.
//! Shared, built-in, workflow, and internal agents are refused.

use std::path::PathBuf;

use super::{
    agent_picker::AgentAccess, App, ComposerMode, Entry, InlineChoice, InlineChoiceOption,
    InlineChoicePending, UiPicker,
};
use crate::agent::{AgentCatalog, AgentOrigin};

/// The agent file a pending delete confirmation will remove.
#[derive(Debug)]
pub(super) struct AgentDeleteTarget {
    pub(super) id: String,
    pub(super) origin: AgentOrigin,
    pub(super) path: PathBuf,
}

/// The file deleting `id` would remove, when it is one of the user's agents.
/// Internal agents are not in `find`, so they are never deletable.
fn delete_target(catalog: &AgentCatalog, id: &str) -> Option<AgentDeleteTarget> {
    let entry = catalog.find(id).ok()?;
    let editable = AgentAccess::of(entry.metadata.origin) == AgentAccess::Editable;
    let path = entry.metadata.path.clone().filter(|_| editable)?;
    Some(AgentDeleteTarget {
        id: id.to_string(),
        origin: entry.metadata.origin,
        path,
    })
}

impl App {
    /// Handles Delete in the `/agents` picker.
    pub(super) fn prompt_delete_selected_agent(&mut self) -> anyhow::Result<()> {
        let Some(id) = self.selected_view_agent_id() else {
            return Ok(());
        };
        let catalog = match AgentCatalog::discover(&self.info.runtime.cwd) {
            Ok(catalog) => catalog,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not load agents: {error}")));
                self.set_status("agent load failed");
                return Ok(());
            }
        };
        let Some(target) = delete_target(&catalog, &id) else {
            self.set_status("only your agents can be deleted");
            return Ok(());
        };
        let choice = InlineChoice::new(
            format!("Delete agent {id}?"),
            format!(
                "Removes {}. This cannot be undone.",
                crate::paths::display(&target.path)
            ),
            vec![
                InlineChoiceOption::available(
                    "delete",
                    'd',
                    "Delete",
                    "Permanently remove this agent definition",
                ),
                InlineChoiceOption::available(
                    "cancel",
                    'c',
                    "Cancel",
                    "Keep the agent and return to the picker",
                )
                .with_alternate_shortcut('n'),
            ],
        )?;
        self.open_choice_over_picker(
            choice,
            InlineChoicePending::DeleteAgent(target),
            "confirm delete agent",
        )
    }

    /// Answers an agent delete confirmation. Any value but `delete` returns
    /// to the agents picker unchanged.
    pub(super) fn submit_delete_agent_choice(
        &mut self,
        value: &str,
        target: AgentDeleteTarget,
        parent: Option<Box<UiPicker>>,
    ) {
        if value != "delete" {
            self.restore_agent_delete_parent(parent);
            return;
        }
        let cursor = parent.as_deref().map(UiPicker::cursor);
        // Re-authorize: the directory may have changed behind the prompt.
        let removed = crate::agent::authorize_existing_agent_file(
            target.origin,
            &target.path,
            &self.info.runtime.cwd,
            crate::paths::home_dir().as_deref(),
        )
        .map_err(anyhow::Error::from)
        .and_then(|_| std::fs::remove_file(&target.path).map_err(anyhow::Error::from));
        if let Err(error) = removed {
            self.insert_entry(&Entry::Error(format!(
                "could not delete agent {}: {error}",
                target.id
            )));
            self.restore_agent_delete_parent(parent);
            self.set_status("agent delete failed");
            return;
        }
        self.insert_entry(&Entry::Notice(format!(
            "deleted agent {}: {}",
            target.id,
            crate::paths::display(&target.path)
        )));
        // Rebuild so the row disappears (or a shadowed definition shows). A
        // failed reload reports itself and leaves the composer on input.
        let _ = self.execute_agents_command();
        let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
            return;
        };
        if let Some(cursor) = cursor {
            picker.restore_cursor(&cursor);
        }
        let still_listed = picker.items.iter().any(|item| item.value == target.id);
        self.set_status(if still_listed {
            format!(
                "deleted agent {}; another definition now applies",
                target.id
            )
        } else {
            format!("deleted agent {}", target.id)
        });
    }

    /// Returns to the agents picker after a cancelled or failed delete.
    pub(super) fn restore_agent_delete_parent(&mut self, parent: Option<Box<UiPicker>>) {
        if !self.restore_choice_parent(parent) {
            self.input_ui.set_composer(ComposerMode::Input);
        }
    }

    fn selected_view_agent_id(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_view_agent() => {
                picker.selected_item().map(|item| item.value.clone())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "agent_delete_tests.rs"]
mod tests;
