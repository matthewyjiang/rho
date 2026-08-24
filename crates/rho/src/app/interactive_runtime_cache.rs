//! Prompt-cache tracking for advertised tool-list changes.

use super::InteractiveRuntime;

impl InteractiveRuntime {
    /// Records whether the advertised tool list differs from the last submitted
    /// request. A changed list busts the prompt-cache prefix even when the
    /// system prompt stays put. Toggling back to the previous list clears it.
    pub(super) fn remember_tool_list(&mut self) {
        self.tool_list_changed = self.cached_tool_specs != self.tools.specs();
    }

    /// True when the advertised tool list differs from the last submitted
    /// request. Sampling updates the submitted baseline.
    pub(crate) fn take_tool_list_changed(&mut self) -> bool {
        let changed = std::mem::take(&mut self.tool_list_changed);
        if changed {
            self.cached_tool_specs = self.tools.specs();
        }
        changed
    }
}
