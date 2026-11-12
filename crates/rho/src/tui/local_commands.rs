use {crate::commands::CommandInvocation, crate::export};

use super::{App, Entry};

impl App {
    pub(super) fn execute_export_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        if self.info.session.no_save {
            self.set_status(
                "session export unavailable with --no-save; use /copy to copy an answer",
            );
            return Ok(());
        }
        let Some(session_id) = self.current_session_id() else {
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
        if self.info.session.no_save {
            self.set_status("session titles unavailable with --no-save");
            return Ok(());
        }
        let title = invocation.args.trim();
        if title.is_empty() {
            self.set_status("usage: /title <name>");
            return Ok(());
        }
        if self.current_session_id().is_none() {
            self.set_status("no active session to rename; send a message first");
            return Ok(());
        }
        // The daemon holds the session's file; its `TitleChanged` reports the
        // rename, and it never generates over a set title.
        if let Err(error) = self.info.session.daemon.rename(title.to_owned()) {
            self.insert_entry(&Entry::Error(format!("could not rename session: {error}")));
            self.set_status("rename failed");
        }
        Ok(())
    }
}
