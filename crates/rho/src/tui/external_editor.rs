use std::{env, ffi::OsString, fs, io::Write, path::Path};

use anyhow::{anyhow, Context};
use ratatui::DefaultTerminal;
use tokio::process::Command;

use super::{App, ComposerMode};

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
        let Some(editor) = resolve_editor(env::var_os("VISUAL"), env::var_os("EDITOR")) else {
            self.notify_status("EDITOR is not set");
            return Ok(());
        };
        let (mut command, path) = match prepare_editor(&editor, &composer_text) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.notify_status(format!("editor failed: {error}"));
                return Ok(());
            }
        };

        let mut terminal_session = self
            .terminal_session
            .take()
            .context("terminal session is unavailable")?;
        let suspended_run = terminal_session
            .run_suspended(terminal, "Opening editor…", || async move {
                #[cfg(unix)]
                let _signal_guard = unix_editor_signals::EditorSignalGuard::install(&mut command)
                    .context("could not prepare editor signal handling")?;
                let status = command.status().await.context("could not start editor")?;
                if !status.success() {
                    return Err(anyhow!("editor exited with {status}"));
                }
                let text =
                    fs::read_to_string(&path).context("could not read edited composer file")?;
                Ok(remove_editor_final_line_ending(text))
            })
            .await;
        self.terminal_session = Some(terminal_session);

        if let Err(resume_error) = suspended_run.resume_result {
            let recovery_text = suspended_run
                .operation_result
                .as_ref()
                .map_or(composer_text.as_str(), String::as_str);
            let recovery_path = preserve_draft_for_recovery(recovery_text).map_err(|error| {
                anyhow!(
                    "{resume_error:#}; also failed to preserve composer for recovery: {error:#}"
                )
            })?;
            let mut failure = resume_error.context(format!(
                "composer saved for recovery at {}",
                recovery_path.display()
            ));
            if let Err(operation_error) = suspended_run.operation_result {
                failure = failure.context(format!(
                    "external editor operation also failed: {operation_error:#}"
                ));
            }
            return Err(failure);
        }
        match suspended_run.operation_result {
            Ok(text) => {
                self.replace_composer_from_editor(text);
                self.status = "composer updated from editor".into();
            }
            Err(error) => self.notify_status(format!("editor failed: {error}")),
        }
        self.ctrl_c_streak = 0;
        self.input_ui.clear_paste_burst();
        Ok(())
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

fn prepare_editor(
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

/// Prefer `VISUAL`, then `EDITOR`. Do not invent a platform default editor.
fn resolve_editor(visual: Option<OsString>, editor: Option<OsString>) -> Option<OsString> {
    match visual {
        Some(value) if !value.is_empty() => Some(value),
        _ => editor.filter(|value| !value.is_empty()),
    }
}

fn remove_editor_final_line_ending(mut text: String) -> String {
    if text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if text.ends_with(['\n', '\r']) {
        text.pop();
    }
    text
}

fn editor_command(editor: &std::ffi::OsStr) -> anyhow::Result<Command> {
    let parts = editor_parts(editor)?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| anyhow!("editor command is empty"))?;
    if program.is_empty() {
        return Err(anyhow!("editor command is empty"));
    }
    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

fn editor_parts(editor: &std::ffi::OsStr) -> anyhow::Result<Vec<OsString>> {
    if editor.is_empty() {
        return Err(anyhow!("editor command is empty"));
    }
    if Path::new(editor).is_file() {
        return Ok(vec![editor.to_os_string()]);
    }
    split_editor_command(editor)
}

#[cfg(unix)]
fn split_editor_command(editor: &std::ffi::OsStr) -> anyhow::Result<Vec<OsString>> {
    let editor = editor
        .to_str()
        .context("editor command is not valid UTF-8 and is not an executable path")?;
    shell_words::split(editor)
        .context("editor command has invalid quoting")
        .map(|parts| parts.into_iter().map(OsString::from).collect())
}

#[cfg(windows)]
fn split_editor_command(editor: &std::ffi::OsStr) -> anyhow::Result<Vec<OsString>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};

    let mut command_line = editor.encode_wide().collect::<Vec<_>>();
    command_line.push(0);
    let mut count = 0;
    let arguments = unsafe { CommandLineToArgvW(command_line.as_ptr(), &mut count) };
    if arguments.is_null() {
        return Err(std::io::Error::last_os_error()).context("could not parse editor command");
    }

    let pointers = unsafe { std::slice::from_raw_parts(arguments, count as usize) };
    let parts = pointers
        .iter()
        .map(|argument| {
            let argument = *argument;
            let mut len = 0;
            while unsafe { *argument.add(len) } != 0 {
                len += 1;
            }
            let value = unsafe { std::slice::from_raw_parts(argument, len) };
            OsString::from_wide(value)
        })
        .collect();
    unsafe {
        LocalFree(arguments.cast());
    }
    Ok(parts)
}

#[cfg(unix)]
mod unix_editor_signals {
    use std::{io, mem::MaybeUninit, os::unix::process::CommandExt};

    use tokio::process::Command;

    const PARENT_IGNORED_SIGNALS: [libc::c_int; 2] = [libc::SIGINT, libc::SIGQUIT];
    const CHILD_DEFAULT_SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP];

    pub(super) struct EditorSignalGuard {
        previous: Vec<(libc::c_int, libc::sigaction)>,
    }

    impl EditorSignalGuard {
        pub(super) fn install(command: &mut Command) -> io::Result<Self> {
            let mut previous = Vec::with_capacity(PARENT_IGNORED_SIGNALS.len());
            for signal in PARENT_IGNORED_SIGNALS {
                match replace_handler(signal, libc::SIG_IGN) {
                    Ok(action) => previous.push((signal, action)),
                    Err(error) => {
                        restore_handlers(&previous);
                        return Err(error);
                    }
                }
            }
            unsafe {
                command.as_std_mut().pre_exec(|| {
                    for signal in CHILD_DEFAULT_SIGNALS {
                        replace_handler(signal, libc::SIG_DFL)?;
                    }
                    Ok(())
                });
            }
            Ok(Self { previous })
        }
    }

    impl Drop for EditorSignalGuard {
        fn drop(&mut self) {
            restore_handlers(&self.previous);
        }
    }

    fn replace_handler(
        signal: libc::c_int,
        handler: libc::sighandler_t,
    ) -> io::Result<libc::sigaction> {
        let mut replacement = unsafe { std::mem::zeroed::<libc::sigaction>() };
        replacement.sa_sigaction = handler;
        unsafe {
            libc::sigemptyset(&mut replacement.sa_mask);
        }
        let mut previous = MaybeUninit::<libc::sigaction>::uninit();
        if unsafe { libc::sigaction(signal, &replacement, previous.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { previous.assume_init() })
    }

    fn restore_handlers(previous: &[(libc::c_int, libc::sigaction)]) {
        for (signal, action) in previous.iter().rev() {
            unsafe {
                libc::sigaction(*signal, action, std::ptr::null_mut());
            }
        }
    }
}

#[cfg(test)]
#[path = "external_editor_tests.rs"]
mod tests;
