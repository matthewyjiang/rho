use crate::{commands::CommandInvocation, export, tool::ToolDisplayStyle};

use super::{doctor, local_diff, App, Entry, ToolEntry, ToolEntryState};

impl App {
    pub(super) fn execute_diff_command(&mut self) -> anyhow::Result<()> {
        let diff = match local_diff::collect(&self.info.cwd) {
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
        let Some(session_id) = self.info.session_id.clone() else {
            self.insert_entry(&Entry::Notice(
                "no active session to export; send a message first".into(),
            ));
            self.status = "nothing to export".into();
            return Ok(());
        };
        match export::write_session_html(&self.info.cwd, &session_id, &invocation.args) {
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

    pub(super) fn execute_doctor_command(&mut self) -> anyhow::Result<()> {
        let config_path = self.info.config_repository.configured_path()?;
        let session_root = crate::paths::rho_dir()?.join("sessions");
        let picker = doctor::picker(doctor::DoctorContext {
            provider: &self.info.provider,
            model: &self.info.model,
            auth: &self.info.auth,
            available_auths: &self.available_auths,
            credential_store: self.credential_store.as_ref(),
            config_path: &config_path,
            session_root: &session_root,
            herdr_enabled: self.info.herdr.is_enabled(),
            herdr_socket_reachable: self.info.herdr.socket_is_reachable(),
        });
        self.composer = super::ComposerMode::Picker(picker);
        self.status = "doctor diagnostics".into();
        Ok(())
    }
}
