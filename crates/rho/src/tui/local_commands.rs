use std::time::Instant;

use ratatui::DefaultTerminal;
use {
    crate::commands::CommandInvocation,
    crate::export,
    rho_tools::tool_card::{ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus},
};

use super::{doctor, local_diff, App, Entry, Session, ToolEntry};
use crate::doctor::{
    build_report, plan_probes, probe_checks, run_probe, DoctorInputs, DoctorProbeId, DoctorReport,
    HerdrProbe, ProbePlaceholder,
};

impl App {
    pub(super) fn execute_copy_command(&mut self) -> anyhow::Result<()> {
        let Some(text) = last_assistant_text(self.history.entries()) else {
            self.set_status("no assistant message to copy");
            return Ok(());
        };
        let text = text.to_owned();
        self.copy_text(&text, Instant::now());
        Ok(())
    }

    pub(super) fn execute_diff_command(&mut self) -> anyhow::Result<()> {
        let diff = match local_diff::collect(&self.info.runtime.cwd) {
            Ok(diff) => diff,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not show git diff: {error}")));
                self.set_status("git diff unavailable");
                return Ok(());
            }
        };
        let body = if diff.has_changes {
            ToolBody::Diff(diff.rows())
        } else {
            ToolBody::Lines(diff.lines)
        };
        self.insert_entry(&Entry::Tool(ToolEntry::new(
            ToolCard::new(
                ToolStatus::Ok,
                ToolFamily::FileCommand,
                ToolHeader::call("diff", None),
            )
            .with_body(body),
            true,
            None,
            None,
        )));
        self.set_status(if diff.has_changes {
            "worktree diff"
        } else {
            "worktree clean"
        });
        Ok(())
    }

    pub(super) fn execute_export_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        let Some(session_id) = self.info.session.session_id.clone() else {
            self.set_status("no active session to export; send a message first");
            return Ok(());
        };
        match export::write_session_export(
            &self.info.runtime.cwd,
            &session_id,
            &export::ExportWriteOptions {
                path_arg: &invocation.args,
                format: None,
                force: false,
            },
        ) {
            Ok(path) => {
                self.set_status(format!("session transcript exported to {}", path.display()));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("unable to export session: {error}")));
                self.set_status("export failed");
            }
        }
        Ok(())
    }

    pub(super) fn execute_title_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        let title = invocation.args.trim();
        if title.is_empty() {
            self.set_status("usage: /title <name>");
            return Ok(());
        }
        let Some(session_id) = self.info.session.session_id.clone() else {
            self.set_status("no active session to rename; send a message first");
            return Ok(());
        };
        // Only cancel pending auto-title after the manual write succeeds so a
        // failed rename leaves generation in place.
        match Session::set_title(&self.info.runtime.cwd, &session_id, title) {
            Ok(updated) => {
                self.pending_session_title = None;
                self.session_title_locked = true;
                self.set_status(format!("session titled: {}", updated.title));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("unable to rename session: {error}")));
                self.set_status("rename failed");
            }
        }
        Ok(())
    }

    pub(super) async fn execute_doctor_command_with_probes(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.set_status("checking provider connections");
        terminal.draw(|frame| self.draw(frame))?;

        let probes = plan_probes(&config, &self.info.runtime.provider, doctor::probe_gate());
        let mut outcomes = Vec::with_capacity(probes.len());
        for id in &probes {
            outcomes.push(run_probe(id.clone(), self.credential_store.clone()).await);
        }
        let mut report = self.doctor_report(&probes, ProbePlaceholder::Checking)?;
        for outcome in &outcomes {
            report.replace_checks(probe_checks(outcome));
        }
        self.open_doctor_picker(&report)
    }

    pub(super) fn execute_doctor_command(&mut self) -> anyhow::Result<()> {
        // During a turn, skip live probes so stream draining is never blocked
        // on a child process or the network.
        let config = self.info.services.config_repository.load()?;
        let probes = plan_probes(&config, &self.info.runtime.provider, doctor::probe_gate());
        let report = self.doctor_report(&probes, ProbePlaceholder::NotChecked)?;
        self.open_doctor_picker(&report)
    }

    fn doctor_report(
        &mut self,
        probes: &[DoctorProbeId],
        placeholder: ProbePlaceholder,
    ) -> anyhow::Result<DoctorReport> {
        self.refresh_available_auths();
        let config_path = self.info.services.config_repository.configured_path()?;
        let session_root = crate::paths::rho_dir()?.join("sessions");
        let clipboard = crate::clipboard::doctor_report();
        Ok(build_report(DoctorInputs {
            provider: &self.info.runtime.provider,
            model: &self.info.runtime.model,
            auth: &self.info.runtime.auth,
            available_auths: &self.available_auths,
            credential_store: self.credential_store.as_ref(),
            config_path: &config_path,
            session_root: &session_root,
            herdr: HerdrProbe::from_reporter(&self.info.services.herdr),
            clipboard: &clipboard,
            mcp_report: &self.mcp_report,
            plugins_report: &self.plugins_report,
            probes,
            placeholder,
        }))
    }

    fn open_doctor_picker(&mut self, report: &DoctorReport) -> anyhow::Result<()> {
        self.input_ui
            .set_composer(super::ComposerMode::Picker(doctor::picker(report)));
        self.set_status("doctor diagnostics");
        Ok(())
    }
}

fn last_assistant_text(entries: &[Entry]) -> Option<&str> {
    entries.iter().rev().find_map(|entry| match entry {
        Entry::Assistant(assistant) if !assistant.text.trim().is_empty() => {
            Some(assistant.text.as_str())
        }
        _ => None,
    })
}

#[cfg(test)]
#[path = "local_commands_tests.rs"]
mod tests;
