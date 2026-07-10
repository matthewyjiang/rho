use crate::tool::ToolDisplayStyle;

use super::{doctor, local_diff, App, Entry, ToolEntry, ToolEntryState};

impl App {
    pub(super) fn execute_diff_command(&mut self) -> anyhow::Result<()> {
        let diff = local_diff::collect(&self.info.cwd)?;
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

    pub(super) fn execute_doctor_command(&mut self) -> anyhow::Result<()> {
        let config_path = self.info.config_repository.configured_path()?;
        let session_root = crate::paths::rho_dir()?.join("sessions");
        let lines = doctor::report(doctor::DoctorContext {
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
        self.insert_entry(&Entry::Notice(lines.join("\n")));
        self.status = "doctor complete".into();
        Ok(())
    }
}
