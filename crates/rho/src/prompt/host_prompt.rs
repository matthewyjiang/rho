//! Instructions a session's host gives its model at system level, such as an
//! ACP host's `systemPrompt`, and how they combine with Rho's own prompt.
//!
//! [`HostPromptMode::Append`] puts the host's text after everything Rho's
//! prompt holds. [`HostPromptMode::Override`] puts it in place of Rho's
//! default instructions: the base prompt, a per-model prompt file, and Rho's
//! tool-usage guidance. It keeps what describes the session's workspace and
//! environment: the working directory, the running model and the
//! advisor's, MCP and OpenAPI context, `AGENTS.md` files, the skill listing,
//! and the instructions an agent definition adds. Neither mode changes the
//! session's tools.

use serde::{Deserialize, Serialize};

/// The host's instructions and how they combine with Rho's prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HostPrompt {
    pub(crate) mode: HostPromptMode,
    pub(crate) text: String,
}

/// How a [`HostPrompt`] combines with Rho's prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HostPromptMode {
    /// After Rho's whole prompt.
    Append,
    /// In place of Rho's default instructions; see the module docs.
    Override,
}

impl HostPrompt {
    /// The text that takes the place of Rho's default instructions, for
    /// [`HostPromptMode::Override`].
    pub(crate) fn override_text(&self) -> Option<&str> {
        match self.mode {
            HostPromptMode::Override => Some(&self.text),
            HostPromptMode::Append => None,
        }
    }

    /// The section that follows Rho's whole prompt, for
    /// [`HostPromptMode::Append`].
    pub(crate) fn appended_section(&self) -> Option<String> {
        match self.mode {
            HostPromptMode::Append => Some(format!("\n\n# Host instructions\n\n{}\n", self.text)),
            HostPromptMode::Override => None,
        }
    }
}
