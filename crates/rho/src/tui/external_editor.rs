use std::{env, fs, io::Write};

use anyhow::{anyhow, Context};
use ratatui::DefaultTerminal;
use tokio::process::Command;

use super::{App, ComposerMode};
#[cfg(unix)]
pub(super) use crate::external_editor::unix_suspended_child_signals;
use crate::external_editor::{editor_command, resolve_editor};

impl App {
    pub(super) fn external_editor_shortcut_matches(&self, key: crossterm::event::KeyEvent) -> bool {
        matches!(self.input_ui.composer(), ComposerMode::Input)
            && self.info.runtime.keybindings.open_editor.matches(key)
    }

    pub(super) async fn open_composer_in_editor(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        // Flush any buffered burst text, then drop enter-suppression so a later
        // submit Enter cannot be treated as a paste newline after resume.
        self.flush_pending_paste_burst();
        self.input_ui.clear_paste_burst();
        let composer_text = self.expanded_input();
        if let Some(text) =
            edit_buffer_in_external_editor(self, terminal, &composer_text, "composer").await?
        {
            self.replace_composer_from_editor(text);
            self.set_status("composer updated from editor");
        }
        self.input_ui.clear_paste_burst();
        Ok(())
    }
}

/// Opens `$VISUAL`/`$EDITOR` on `initial` and returns the edited text.
///
/// Soft failures (no editor, editor error) notify status and return `Ok(None)`.
/// Terminal-resume failures propagate as `Err` after preserving recovery text.
pub(super) async fn edit_buffer_in_external_editor(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    initial: &str,
    recovery_label: &str,
) -> anyhow::Result<Option<String>> {
    let Some(editor) = resolve_editor(env::var_os("VISUAL"), env::var_os("EDITOR")) else {
        app.notify_status("EDITOR is not set");
        return Ok(None);
    };
    let (mut command, path) = match prepare_editor(&editor, initial) {
        Ok(prepared) => prepared,
        Err(error) => {
            app.notify_status(format!("editor failed: {error}"));
            return Ok(None);
        }
    };

    let mut terminal_session = app
        .terminal_session
        .take()
        .context("terminal session is unavailable")?;
    let suspended_run = terminal_session
        .run_suspended(terminal, "Opening editor…", || async move {
            #[cfg(unix)]
            let _signal_guard =
                unix_suspended_child_signals::SuspendedChildSignalGuard::install(&mut command)
                    .context("could not prepare editor signal handling")?;
            let status = command.status().await.context("could not start editor")?;
            if !status.success() {
                return Err(anyhow!("editor exited with {status}"));
            }
            let text = fs::read_to_string(&path).context("could not read edited file")?;
            Ok(remove_editor_final_line_ending(text))
        })
        .await;
    app.terminal_session = Some(terminal_session);

    if let Err(resume_error) = suspended_run.resume_result {
        let recovery_text = suspended_run
            .operation_result
            .as_ref()
            .map_or(initial, String::as_str);
        let recovery_path = preserve_draft_for_recovery(recovery_text).map_err(|error| {
            anyhow!(
                "{resume_error:#}; also failed to preserve {recovery_label} for recovery: {error:#}"
            )
        })?;
        let mut failure = resume_error.context(format!(
            "{recovery_label} saved for recovery at {}",
            recovery_path.display()
        ));
        if let Err(operation_error) = suspended_run.operation_result {
            failure = failure.context(format!(
                "external editor operation also failed: {operation_error:#}"
            ));
        }
        return Err(failure);
    }
    app.ctrl_c_streak = 0;
    match suspended_run.operation_result {
        Ok(text) => Ok(Some(text)),
        Err(error) => {
            app.notify_status(format!("editor failed: {error}"));
            Ok(None)
        }
    }
}

fn preserve_draft_for_recovery(contents: &str) -> anyhow::Result<std::path::PathBuf> {
    let mut file = tempfile::Builder::new()
        .prefix("rho-composer-recovery-")
        .suffix(".md")
        .tempfile()
        .context("could not create composer recovery file")?;
    file.write_all(contents.as_bytes())
        .context("could not write composer recovery file")?;
    file.flush()
        .context("could not flush composer recovery file")?;
    file.into_temp_path()
        .keep()
        .context("could not preserve composer recovery file")
}

pub(super) fn prepare_editor(
    editor: &std::ffi::OsStr,
    contents: &str,
) -> anyhow::Result<(Command, tempfile::TempPath)> {
    let mut command = editor_command(editor)?;
    let mut file = tempfile::Builder::new()
        .prefix("rho-composer-")
        .suffix(".md")
        .tempfile()
        .context("could not create composer file")?;
    file.write_all(contents.as_bytes())
        .context("could not write composer file")?;
    file.flush().context("could not flush composer file")?;
    let path = file.into_temp_path();
    command.arg(path.as_os_str());
    Ok((command, path))
}

pub(super) fn remove_editor_final_line_ending(mut text: String) -> String {
    if text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if text.ends_with(['\n', '\r']) {
        text.pop();
    }
    text
}

#[cfg(test)]
#[path = "external_editor_tests.rs"]
mod tests;
