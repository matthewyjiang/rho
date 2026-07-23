use ratatui::DefaultTerminal;
use rho_providers::{
    model::provider_models::{probe_provider_models, ProviderModelHealth},
    provider::{self, ProviderAuthKind, ProviderModelRefreshKind},
};
use {crate::commands::CommandInvocation, crate::export, rho_tools::tool::ToolDisplayStyle};

use super::{doctor, local_diff, App, Entry, ToolEntry, ToolEntryState};

impl App {
    pub(super) fn execute_diff_command(&mut self) -> anyhow::Result<()> {
        let diff = match local_diff::collect(&self.info.runtime.cwd) {
            Ok(diff) => diff,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("unable to show Git diff: {error}")));
                self.status = "git diff unavailable".into();
                return Ok(());
            }
        };
        self.insert_entry(&Entry::Tool(ToolEntry {
            state: ToolEntryState::Finished {
                ok: true,
                display_style: ToolDisplayStyle::FileDiff,
            },
            display_lines: diff.lines,
            expanded: true,
            image: None,
        }));
        self.status = if diff.has_changes {
            "worktree diff".into()
        } else {
            "worktree clean".into()
        };
        Ok(())
    }

    pub(super) fn execute_export_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        let Some(session_id) = self.info.session.session_id.clone() else {
            self.insert_entry(&Entry::Notice(
                "no active session to export; send a message first".into(),
            ));
            self.status = "nothing to export".into();
            return Ok(());
        };
        match export::write_session_html(&self.info.runtime.cwd, &session_id, &invocation.args) {
            Ok(path) => {
                self.insert_entry(&Entry::Notice(format!(
                    "session transcript exported to {}",
                    path.display()
                )));
                self.status = "session exported".into();
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("unable to export session: {error}")));
                self.status = "export failed".into();
            }
        }
        Ok(())
    }

    pub(super) async fn execute_doctor_command_with_probes(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.status = "checking provider connections".into();
        terminal.draw(|frame| self.draw(frame))?;

        let mut provider_health = Vec::new();
        for descriptor in provider::providers() {
            if descriptor.auth_kind != ProviderAuthKind::None
                || descriptor.model_refresh != Some(ProviderModelRefreshKind::OpenAiCompatible)
            {
                continue;
            }
            let Some(endpoint) = config.resolved_provider_endpoint(descriptor.name) else {
                continue;
            };
            let health =
                probe_provider_models(descriptor.name, &endpoint, self.credential_store.as_ref())
                    .await;
            provider_health.push((descriptor.name.to_string(), health));
        }
        self.open_doctor_picker(&provider_health)
    }

    pub(super) fn execute_doctor_command(&mut self) -> anyhow::Result<()> {
        self.open_doctor_picker(&[])
    }

    fn open_doctor_picker(
        &mut self,
        provider_health: &[(String, ProviderModelHealth)],
    ) -> anyhow::Result<()> {
        let config_path = self.info.services.config_repository.configured_path()?;
        let session_root = crate::paths::rho_dir()?.join("sessions");
        let picker = doctor::picker(doctor::DoctorContext {
            provider: &self.info.runtime.provider,
            model: &self.info.runtime.model,
            auth: &self.info.runtime.auth,
            available_auths: &self.available_auths,
            credential_store: self.credential_store.as_ref(),
            config_path: &config_path,
            session_root: &session_root,
            herdr_enabled: self.info.services.herdr.is_enabled(),
            herdr_socket_reachable: self.info.services.herdr.socket_is_reachable(),
            provider_health,
        });
        self.input_ui.composer = super::ComposerMode::Picker(picker);
        self.status = "doctor diagnostics".into();
        Ok(())
    }
}
