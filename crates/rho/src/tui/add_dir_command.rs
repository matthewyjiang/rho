//! `/add-dir [path]` adds a directory to the session's workspace scope.

use super::{App, CommandInvocation, Entry, InteractiveRuntime};
use crate::app::interactive_runtime::AddDirOutcome;

impl App {
    pub(super) async fn execute_add_dir_command(
        &mut self,
        invocation: &CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let raw = invocation.args.trim();
        if raw.is_empty() {
            let dirs = agent.added_dirs().as_slice();
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
        match agent.add_dir(dir.clone()).await {
            Ok(AddDirOutcome::Added) => {
                self.insert_entry(&Entry::Notice(format!(
                    "added directory {}",
                    crate::paths::display(&dir)
                )));
                self.set_status("directory added");
            }
            Ok(AddDirOutcome::AlreadyCovered(root)) => {
                self.insert_entry(&Entry::Notice(format!(
                    "{} is already in scope through {}",
                    crate::paths::display(&dir),
                    crate::paths::display(&root)
                )));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not add directory: {error}")));
            }
        }
        self.info.runtime.added_dirs = agent.added_dirs().clone();
        Ok(())
    }
}
