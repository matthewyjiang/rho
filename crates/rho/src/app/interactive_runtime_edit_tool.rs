//! File edit tool as a runtime state transition.
//!
//! Swapping the advertised edit surface changes the tool list, which the SDK
//! cannot hot-swap on a live runtime. The change rebuilds the runtime and
//! rebinds the session so it lands on the next turn. Session ID and history
//! survive it.
//!
//! The system prompt stays fixed and format-agnostic. The model learns about the
//! new surface from an appended context notice that carries the live schema.

use super::InteractiveRuntime;

/// Result of a successful mid-session edit-tool switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditToolChange {
    pub(crate) previous: rho_tools::EditFormat,
    pub(crate) display: String,
}

impl InteractiveRuntime {
    /// Swaps the advertised file edit tool for the next turn.
    ///
    /// Keeps the system prompt fixed for prompt-cache stability, rebuilds the
    /// runtime tool list, and appends a model-facing schema notice when the
    /// surface actually changes. Returns [`None`] when this run has no edit tool
    /// or the selection is already active.
    pub(crate) async fn set_edit_tool(
        &mut self,
        edit_tool: rho_tools::EditFormat,
        max_output_bytes: usize,
    ) -> anyhow::Result<Option<EditToolChange>> {
        if self.runs.is_active() {
            anyhow::bail!("edit tool cannot change while a run is active");
        }
        let Some(previous) = self.tools.set_edit_tool(edit_tool, max_output_bytes) else {
            return Ok(None);
        };
        if let Err(error) = self.rebind_current_session().await {
            let _ = self.tools.set_edit_tool(previous, max_output_bytes);
            return Err(error);
        }
        match self.append_edit_tool_switch_notice(previous, edit_tool) {
            Ok(display) => Ok(Some(EditToolChange { previous, display })),
            Err(error) => {
                // Best-effort restore so a notice failure does not leave the
                // session advertising a tool the model was never told about.
                let _ = self.tools.set_edit_tool(previous, max_output_bytes);
                let _ = self.rebind_current_session().await;
                Err(error)
            }
        }
    }

    fn append_edit_tool_switch_notice(
        &mut self,
        previous: rho_tools::EditFormat,
        current: rho_tools::EditFormat,
    ) -> anyhow::Result<String> {
        let spec = self
            .tools
            .specs()
            .into_iter()
            .find(|spec| spec.name == current.tool_name())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "edit tool `{}` is missing after the mid-session switch",
                    current.tool_name()
                )
            })?;
        let (model, display) = crate::prompt::edit_tool_switch_context(previous, current, &spec);
        self.append_user_context_with_display(model, display.clone())?;
        Ok(display)
    }

    pub(crate) fn tool_specs(&self) -> Vec<rho_sdk::model::ToolSpec> {
        self.tools.specs()
    }
}
