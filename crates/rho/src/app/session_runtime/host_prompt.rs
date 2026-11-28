//! The instructions a session's host gave its model (`crate::prompt::HostPrompt`),
//! kept in its file so the daemon reopens it with them: a session an ACP
//! host gave a `systemPrompt` keeps it when the daemon unloads the session
//! and a prompt or an attach without one reopens it
//! (`daemon_host::sessions::setup`).
//!
//! Like the parts left out (`left_out.rs`), the record rides in snapshot
//! metadata and every durable save rewrites it. A session no host gave
//! instructions keeps no record.

use anyhow::Context;
use rho_sdk::{SessionSnapshot, SystemPrompt};

use crate::{
    app::{
        active_prompt::ActivePrompt,
        conversation_switch::replace_system_prompt,
        tools_prompt::{prompt_base, untemplated_prompt, PromptBase},
    },
    model_identity::PromptModel,
    prompt::HostPrompt,
};

/// The snapshot metadata key of a session's [`HostPrompt`], as JSON.
const METADATA_KEY: &str = "rho.host_prompt";

/// The instructions `stored` was last saved with, if any. A record this
/// build cannot read fails, rather than reopen the session without them.
pub(crate) fn saved(stored: &crate::session::Session) -> anyhow::Result<Option<HostPrompt>> {
    let Some(encoded) = stored.session_metadata(METADATA_KEY)? else {
        return Ok(None);
    };
    serde_json::from_str(&encoded)
        .map(Some)
        .context("could not read the system prompt the session's host gave it")
}

/// `snapshot` with `prompt` in its metadata. A host never takes its
/// instructions back, so without any the snapshot keeps what it has.
pub(super) fn decorate(prompt: Option<&HostPrompt>, snapshot: SessionSnapshot) -> SessionSnapshot {
    match prompt {
        Some(prompt) => {
            let encoded = serde_json::to_string(prompt).expect("a host prompt encodes as JSON");
            snapshot.with_metadata(METADATA_KEY, encoded)
        }
        None => snapshot,
    }
}

impl super::SessionRuntime {
    /// Gives the session's model `prompt` as its host's instructions, in
    /// place of any it had, from its next model request, and saves the
    /// session with them. A failure to build the prompt leaves the session
    /// as it was. A failed save keeps the change, which the session's next
    /// durable save writes, as it writes every save's record.
    ///
    /// The model prompt file the session loaded stays, as other refreshes
    /// keep it (`bind_tool_sources`), so an edit to that file waits for a
    /// new session. Only a session an override kept from loading it reads
    /// it now.
    pub(crate) fn set_host_prompt(&mut self, prompt: HostPrompt) -> anyhow::Result<()> {
        if self.build.launch.host_prompt.as_ref() == Some(&prompt) {
            return Ok(());
        }
        let launch = &self.build.launch;
        let (next, template) = match prompt_base(&self.agent, launch.left_out.system_prompt) {
            PromptBase::Untemplated(replacing) => (
                untemplated_prompt(replacing, Some(&prompt), launch.prompt_suffix),
                None,
            ),
            PromptBase::Template(_) => {
                let mut template = self
                    .built
                    .prompt_template
                    .clone()
                    .context("the session's prompt has no template")?;
                template.set_host_prompt(Some(prompt.clone()));
                let running = PromptModel::from_sdk_identity(&self.built.provider.identity());
                let skipped_file = launch
                    .host_prompt
                    .as_ref()
                    .is_some_and(|current| current.override_text().is_some());
                let built = if skipped_file {
                    template.build(&running)?
                } else {
                    template.render(&running, self.built.prompt.loaded.as_ref())
                };
                (ActivePrompt::from_prepared(built), Some(template))
            }
        };
        if let SystemPrompt::Custom(text) = &next.system {
            replace_system_prompt(&self.built.session, text)?;
        }
        if template.is_some() {
            self.built.prompt_template = template;
        }
        self.built
            .prompt
            .adopt(next, &self.built.diagnostics, self.built.tools.advisor());
        self.build.launch.host_prompt = Some(prompt);
        self.save_state()
            .context("gave the session the system prompt, but could not save it")
    }
}
