use std::time::Instant;

use {
    crate::commands::CommandInvocation,
    crate::export,
    rho_tools::tool_card::{ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus},
};

use super::{local_diff, App, Entry, Session, ToolEntry};

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
