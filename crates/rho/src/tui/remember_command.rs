//! `/remember [global] <text>` writes a standing instruction without a model turn.

use std::{fs, io::ErrorKind};

use super::{statusline::path::compact_cwd, App, CommandInvocation, Entry, InteractiveRuntime};

const USAGE: &str = "usage: /remember [global] <text>";

impl App {
    pub(super) fn execute_remember_command(
        &mut self,
        invocation: &CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let args = invocation.args.trim();
        let mut parts = args.splitn(2, char::is_whitespace);
        let global = parts.next().is_some_and(|part| part == "global");
        let text = if global {
            parts.next().unwrap_or_default().trim()
        } else {
            args
        };
        // Collapsed pastes are expanded before dispatch. Keep memories to one
        // line, just like the slash command parser, rather than writing a marker.
        if text.is_empty() || text.contains(['\n', '\r']) {
            self.set_status(USAGE);
            return Ok(());
        }

        let path = if global {
            let Some(home) = crate::paths::home_dir() else {
                self.insert_entry(&Entry::Error(
                    "could not remember instruction: home directory unavailable".into(),
                ));
                self.set_status("remember failed");
                return Ok(());
            };
            crate::paths::user_agents_md(&home)
        } else {
            // This is the first project AGENTS.md loaded by prompt discovery:
            // Git root when present, otherwise the current working directory.
            crate::workspace::project_ancestor_dirs(&self.info.runtime.cwd)[0].join("AGENTS.md")
        };
        let display_path = compact_cwd(&path);
        let saved = (|| -> anyhow::Result<()> {
            let existing = match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    anyhow::ensure!(metadata.is_file(), "destination is not a regular file");
                    Some(fs::read_to_string(&path)?)
                }
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            let contents = crate::agents_md::appended_contents(existing.as_deref(), text);
            if existing.is_some() {
                // Preserve existing permissions and refuse to replace a symlink.
                crate::config_writer::replace_regular_file_atomically(&path, contents.as_bytes())?;
            } else {
                // The atomic writer also creates missing parent directories.
                crate::config_writer::write_atomically(&path, &contents)?;
            }
            Ok(())
        })();
        if let Err(error) = saved {
            self.insert_entry(&Entry::Error(format!(
                "could not remember instruction in {display_path}: {error}"
            )));
            self.set_status("remember failed");
            return Ok(());
        }

        let display = format!("remembered in {display_path}");
        let context = format!(
            "The user added this standing instruction to {}. Apply it in this session:\n- {text}",
            crate::paths::prompt_data(&path),
        );
        self.insert_entry(&Entry::Notice(display.clone()));
        if let Err(error) = agent.append_user_context_with_display(context, display) {
            // The file is already durable. Do not report a write failure or
            // silently imply that the instruction reached the live session.
            self.insert_entry(&Entry::Error(format!(
                "could not apply remembered instruction to this session: {error}"
            )));
            self.set_status("instruction saved; session update failed");
            return Ok(());
        }
        self.set_status("instruction remembered");
        Ok(())
    }
}
