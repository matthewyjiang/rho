//! `/add-dir [path]` adds a directory to the session's workspace scope.

use super::{App, CommandInvocation, Entry};

impl App {
    pub(super) fn execute_add_dir_command(
        &mut self,
        invocation: &CommandInvocation,
    ) -> anyhow::Result<()> {
        let raw = invocation.args.trim();
        if raw.is_empty() {
            // Kept current by the session's options.
            let dirs = self.info.runtime.added_dirs.as_slice();
            let listing = if dirs.is_empty() {
                "none".to_string()
            } else {
                dirs.iter()
                    .map(|dir| crate::paths::display(dir))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            self.insert_entry(&Entry::Notice(format!(
                "added directories: {listing}. usage: /add-dir <path>"
            )));
            return Ok(());
        }
        let dir = match crate::added_dirs::resolve(
            std::path::Path::new(raw),
            &self.info.runtime.cwd,
            crate::paths::home_dir().as_deref(),
        ) {
            Ok(dir) => dir,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not add directory: {error}")));
                return Ok(());
            }
        };
        // The daemon's notice says it was added; a refusal says why not.
        self.configure_daemon(rho_protocol::SessionChange::AddDirectory { path: dir })
    }
}
