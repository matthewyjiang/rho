pub(super) use crate::config::default_inline_shell as default_shell;

use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
};

use tokio::process::Command;

const INLINE_SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InlineShellMode {
    IncludeInContext,
    ExcludeFromContext,
}

impl InlineShellMode {
    pub(super) fn parse(input: &str) -> Option<(Self, &str)> {
        if let Some(command) = input.strip_prefix("!!") {
            Some((Self::ExcludeFromContext, command.trim()))
        } else {
            input
                .strip_prefix('!')
                .map(|command| (Self::IncludeInContext, command.trim()))
        }
    }

    pub(super) const fn included_in_context(self) -> bool {
        matches!(self, Self::IncludeInContext)
    }
}

pub(super) fn mode(input: &str) -> Option<InlineShellMode> {
    InlineShellMode::parse(input).map(|(mode, _)| mode)
}

pub(super) fn mode_when_idle(running: bool, input: &str) -> Option<InlineShellMode> {
    (!running).then(|| mode(input)).flatten()
}

pub(super) fn mode_hint_when_idle(
    running: bool,
    input: &str,
) -> Option<(&'static str, ratatui::style::Style)> {
    (!running).then(|| mode_hint(input)).flatten()
}

pub(super) fn mode_hint(input: &str) -> Option<(&'static str, ratatui::style::Style)> {
    match mode(input)? {
        InlineShellMode::IncludeInContext => Some((
            "SHELL - output will be included in model context",
            super::Theme::shell_context(),
        )),
        InlineShellMode::ExcludeFromContext => Some((
            "LOCAL SHELL - output will not be included in model context",
            super::Theme::shell_local(),
        )),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ShellOutput {
    pub(super) shell: String,
    pub(super) command: String,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) exit_code: String,
    pub(super) ok: bool,
}

pub(super) fn available_shells(selected: &str) -> Vec<String> {
    let candidates: &[&str] = if cfg!(windows) {
        &["powershell", "pwsh", "cmd"]
    } else {
        &["bash", "zsh", "fish", "sh"]
    };
    let mut shells = candidates
        .iter()
        .filter(|shell| executable_exists(shell))
        .map(|shell| (*shell).to_string())
        .collect::<Vec<_>>();
    if !selected.is_empty() && !shells.iter().any(|shell| shell == selected) {
        shells.push(selected.to_string());
    }
    shells
}

pub(super) async fn execute(
    shell: &str,
    command: &str,
    cwd: &Path,
) -> std::io::Result<ShellOutput> {
    let mut process = Command::new(shell);
    match executable_name(shell).to_ascii_lowercase().as_str() {
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => {
            process.args(["-NoLogo", "-NoProfile", "-Command", command]);
        }
        "cmd" | "cmd.exe" => {
            process.args(["/C", command]);
        }
        "sh" | "sh.exe" => {
            process.args(["-c", command]);
        }
        _ => {
            process.args(["-lc", command]);
        }
    }
    let output = tokio::time::timeout(
        INLINE_SHELL_TIMEOUT,
        process
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "inline shell command timed out after {} seconds",
                INLINE_SHELL_TIMEOUT.as_secs()
            ),
        )
    })??;
    Ok(ShellOutput {
        shell: shell.to_string(),
        command: command.to_string(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output
            .status
            .code()
            .map_or_else(|| "signal".into(), |code| code.to_string()),
        ok: output.status.success(),
    })
}

pub(super) fn context_text(output: &ShellOutput) -> String {
    format!(
        "Inline shell command executed with {}:\n```shell\n{}\n```\nstdout:\n```text\n{}\n```\nstderr:\n```text\n{}\n```\nexit code: {}",
        output.shell,
        output.command,
        output.stdout,
        output.stderr,
        output.exit_code
    )
}

pub(super) fn display_text(output: &ShellOutput, included_in_context: bool) -> String {
    display_lines(output, included_in_context).join("\n")
}

pub(super) fn display_lines(output: &ShellOutput, included_in_context: bool) -> Vec<String> {
    let context = if included_in_context {
        "included in context"
    } else {
        "excluded from context"
    };
    let mut lines = vec![
        format!("{} {}", output.shell, output.command),
        format!("{context}  exit code: {}", output.exit_code),
    ];
    if !output.stdout.is_empty() {
        lines.push(String::new());
        lines.push(output.stdout.trim_end().to_string());
    }
    if !output.stderr.is_empty() {
        lines.push(String::new());
        lines.push(format!("stderr:\n{}", output.stderr.trim_end()));
    }
    lines
}

fn executable_exists(executable: &str) -> bool {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return path.is_file();
    }
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths)
            .any(|directory| executable_paths(&directory, executable).any(|path| path.is_file()))
    })
}

fn executable_paths(directory: &Path, executable: &str) -> impl Iterator<Item = PathBuf> {
    let mut paths = vec![directory.join(executable)];
    if cfg!(windows) && Path::new(executable).extension().is_none() {
        paths.push(directory.join(format!("{executable}.exe")));
        paths.push(directory.join(format!("{executable}.cmd")));
        paths.push(directory.join(format!("{executable}.bat")));
    }
    paths.into_iter()
}

fn executable_name(shell: &str) -> &str {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell)
}

impl super::App {
    pub(super) fn block_inline_shell_during_turn(&mut self) -> anyhow::Result<()> {
        self.insert_entry(&super::Entry::Notice(
            "inline shell is unavailable while a model turn is running".into(),
        ));
        self.status = "inline shell unavailable while running".into();
        Ok(())
    }

    pub(super) fn block_pasted_inline_shell(&mut self) -> anyhow::Result<()> {
        self.insert_entry(&super::Entry::Error(
            "inline shell commands cannot start from collapsed pasted content".into(),
        ));
        self.status = "inline shell paste blocked".into();
        Ok(())
    }

    pub(super) fn clear_submitted_input(&mut self) {
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.clamp_command_selection();
    }

    pub(super) fn inline_shell_picker_is_open(&self) -> bool {
        matches!(
            &self.composer,
            super::ComposerMode::Picker(picker)
                if picker.action == super::PickerAction::Config
                    && picker.items.iter().any(|item| item.value.starts_with(super::config_picker::INLINE_SHELL_PREFIX))
        )
    }

    pub(super) async fn execute_inline_shell(
        &mut self,
        mode: InlineShellMode,
        command: String,
        terminal: &mut ratatui::DefaultTerminal,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.running {
            return self.block_inline_shell_during_turn();
        }
        if command.is_empty() {
            self.status = "enter a shell command after ! or !!".into();
            return Ok(());
        }
        let config = self.info.config_repository.load()?;
        let shell = if config.inline_shell.trim().is_empty() {
            default_shell()
        } else {
            config.inline_shell
        };
        self.push_input_history(&format!(
            "{}{}",
            if mode.included_in_context() {
                "!"
            } else {
                "!!"
            },
            command
        ));
        self.status = format!("running {shell}");
        terminal.draw(|frame| self.draw(frame))?;
        let output = match execute(&shell, &command, &self.info.cwd).await {
            Ok(output) => output,
            Err(error) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "could not run inline shell with {shell}: {error}"
                )));
                self.status = "inline shell failed".into();
                return Ok(());
            }
        };
        let context = crate::tool::truncate(context_text(&output), config.max_output_bytes);
        let persisted_display = crate::tool::truncate(
            format!(
                "!{command}\n\n{}",
                display_text(&output, /*included_in_context*/ true)
            ),
            config.max_output_bytes,
        );
        if mode.included_in_context() {
            self.ensure_session(agent)?;
            agent.append_user_context_with_display(context, persisted_display)?;
        }
        let display_text = crate::tool::truncate(
            display_text(&output, mode.included_in_context()),
            config.max_output_bytes,
        );
        let display_lines = display_text.lines().map(str::to_string).collect();
        self.insert_entry(&super::Entry::Tool(super::ToolEntry {
            state: super::ToolEntryState::Finished {
                ok: output.ok,
                display_style: crate::tool::ToolDisplayStyle::file_or_command(),
            },
            display_lines,
            expanded: true,
        }));
        self.statusline.refresh_git_branch();
        self.status = if output.ok {
            if mode.included_in_context() {
                "shell output included in context".into()
            } else {
                "shell output excluded from context".into()
            }
        } else {
            format!("shell exited with {}", output.exit_code)
        };
        Ok(())
    }
}

#[cfg(test)]
#[path = "inline_shell_tests.rs"]
mod tests;
