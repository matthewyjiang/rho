pub(super) use crate::config::default_inline_shell as default_shell;

use std::{path::Path, process::Stdio};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::mpsc,
};

const INLINE_SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InlineShellMode {
    IncludeInContext,
    ExcludeFromContext,
}

impl InlineShellMode {
    /// Parse a history or pasted entry that still uses the `!` / `!!` prefix form.
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

    pub(super) const fn history_prefix(self) -> &'static str {
        match self {
            Self::IncludeInContext => "!",
            Self::ExcludeFromContext => "!!",
        }
    }
}

/// Compact top-divider labels for shell mode, longest first for width fitting.
pub(super) fn mode_divider_labels(mode: InlineShellMode) -> &'static [&'static str] {
    match mode {
        InlineShellMode::IncludeInContext => {
            &["shell · included in context", "shell · in context", "shell"]
        }
        InlineShellMode::ExcludeFromContext => &[
            "shell · excluded from context",
            "shell · not in context",
            "shell",
        ],
    }
}

pub(super) struct PendingShellTask {
    mode: InlineShellMode,
    max_output_bytes: usize,
    shell: String,
    command: String,
    stdout: String,
    stderr: String,
    updates: mpsc::UnboundedReceiver<ShellStreamUpdate>,
    handle: tokio::task::JoinHandle<std::io::Result<ShellOutput>>,
}

#[derive(Clone, Copy)]
enum ShellStreamKind {
    Stdout,
    Stderr,
}

struct ShellStreamUpdate {
    kind: ShellStreamKind,
    text: String,
}

pub(super) struct DeferredShellContext {
    context: String,
    persisted_display: String,
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
        .filter(|shell| crate::executable::find_on_path(shell).is_some())
        .map(|shell| (*shell).to_string())
        .collect::<Vec<_>>();
    if !selected.is_empty() && !shells.iter().any(|shell| shell == selected) {
        shells.push(selected.to_string());
    }
    shells
}

#[cfg(test)]
pub(super) async fn execute(
    shell: &str,
    command: &str,
    cwd: &Path,
) -> std::io::Result<ShellOutput> {
    execute_streaming(shell, command, cwd, None).await
}

async fn execute_streaming(
    shell: &str,
    command: &str,
    cwd: &Path,
    updates: Option<mpsc::UnboundedSender<ShellStreamUpdate>>,
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
    let mut child = process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout configured as piped");
    let stderr = child.stderr.take().expect("stderr configured as piped");
    let stdout_updates = updates.clone();
    let stdout_reader = read_stream(stdout, ShellStreamKind::Stdout, stdout_updates);
    let stderr_reader = read_stream(stderr, ShellStreamKind::Stderr, updates);
    let wait = async {
        match tokio::time::timeout(INLINE_SHELL_TIMEOUT, child.wait()).await {
            Ok(status) => status,
            Err(_) => {
                child.kill().await?;
                let _ = child.wait().await;
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "inline shell command timed out after {} seconds",
                        INLINE_SHELL_TIMEOUT.as_secs()
                    ),
                ))
            }
        }
    };
    let (stdout, stderr, status) = tokio::join!(stdout_reader, stderr_reader, wait);
    let status = status?;
    Ok(ShellOutput {
        shell: shell.to_string(),
        command: command.to_string(),
        stdout: stdout?,
        stderr: stderr?,
        exit_code: status
            .code()
            .map_or_else(|| "signal".into(), |code| code.to_string()),
        ok: status.success(),
    })
}

async fn read_stream(
    mut stream: impl AsyncRead + Unpin,
    kind: ShellStreamKind,
    updates: Option<mpsc::UnboundedSender<ShellStreamUpdate>>,
) -> std::io::Result<String> {
    let mut output = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..read]);
        if let Some(updates) = &updates {
            let _ = updates.send(ShellStreamUpdate {
                kind,
                text: String::from_utf8_lossy(&buffer[..read]).into_owned(),
            });
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
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

pub(super) fn display_lines(output: &ShellOutput, _included_in_context: bool) -> Vec<String> {
    let prompt = match output.shell.to_ascii_lowercase().as_str() {
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => "PS",
        _ => "$",
    };
    let mut lines = vec![format!("{prompt} {}", output.command)];
    if !output.stdout.is_empty() {
        lines.push(String::new());
        lines.push(output.stdout.trim_end().to_string());
    }
    lines
}

fn executable_name(shell: &str) -> &str {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell)
}

impl super::App {
    pub(super) fn start_inline_shell(
        &mut self,
        mode: InlineShellMode,
        command: String,
    ) -> anyhow::Result<()> {
        if command.is_empty() {
            self.status = "enter a shell command after ! or !!".into();
            return Ok(());
        }
        let config = self.info.services.config_repository.load()?;
        let shell = if config.inline_shell.trim().is_empty() {
            default_shell()
        } else {
            config.inline_shell
        };
        self.push_input_history(&format!("{}{command}", mode.history_prefix()));
        let cwd = self.info.runtime.cwd.clone();
        let task_shell = shell.clone();
        let task_command = command.clone();
        let (updates_tx, updates_rx) = mpsc::unbounded_channel();
        self.pending_inline_shells.push(PendingShellTask {
            mode,
            max_output_bytes: config.max_output_bytes,
            shell: shell.clone(),
            command: command.clone(),
            stdout: String::new(),
            stderr: String::new(),
            updates: updates_rx,
            handle: tokio::spawn(async move {
                execute_streaming(&task_shell, &task_command, &cwd, Some(updates_tx)).await
            }),
        });
        self.status = format!("running {shell}");
        Ok(())
    }

    pub(super) fn cancel_inline_shells(&mut self) -> bool {
        if self.pending_inline_shells.is_empty() {
            return false;
        }
        for mut task in std::mem::take(&mut self.pending_inline_shells) {
            task.drain_updates();
            task.handle.abort();
            let output = ShellOutput {
                shell: task.shell,
                command: task.command,
                stdout: task.stdout,
                stderr: task.stderr,
                exit_code: "cancelled".into(),
                ok: false,
            };
            self.insert_entry(&super::Entry::Tool(super::ToolEntry {
                state: super::ToolEntryState::Finished {
                    ok: false,
                    display_style: rho_tools::tool::ToolDisplayStyle::file_or_command(),
                },
                display_lines: display_lines(&output, task.mode.included_in_context()),
                expanded: true,
                image: None,
            }));
        }
        self.status = "inline shell cancelled".into();
        true
    }

    /// Leave shell mode and restore a normal composer, keeping the typed command text.
    pub(super) fn exit_shell_mode(&mut self) -> bool {
        if self.shell_mode.take().is_none() {
            return false;
        }
        self.status = self.busy_status_label().into();
        true
    }

    /// Enter or upgrade shell mode from a leading `!` keypress.
    ///
    /// Shell mode is explicit App state. The composer stores only the command
    /// text, so cursor/home/delete/word/paste coordinates stay ordinary.
    pub(super) fn try_enter_shell_mode_from_bang(&mut self) -> bool {
        if !matches!(self.composer, super::ComposerMode::Input)
            || self.input_cursor != 0
            || !self.input.is_empty()
            || !self.paste_segments.is_empty()
        {
            return false;
        }
        match self.shell_mode {
            None => {
                self.shell_mode = Some(InlineShellMode::IncludeInContext);
                true
            }
            Some(InlineShellMode::IncludeInContext) => {
                self.shell_mode = Some(InlineShellMode::ExcludeFromContext);
                true
            }
            Some(InlineShellMode::ExcludeFromContext) => true,
        }
    }

    pub(super) fn shell_submission(&self) -> Option<(InlineShellMode, String)> {
        if let Some(mode) = self.shell_mode {
            return Some((mode, self.input.trim().to_string()));
        }
        InlineShellMode::parse(self.input.trim()).map(|(mode, command)| (mode, command.to_string()))
    }

    /// Restore composer text that may still use the historical `!` / `!!` prefix form.
    pub(super) fn apply_composer_text(
        &mut self,
        text: String,
        paste_segments: Vec<super::PasteSegment>,
        submission_mode: super::InputSubmissionMode,
    ) {
        if paste_segments.is_empty() {
            if let Some((mode, command)) = InlineShellMode::parse(text.trim_end()) {
                self.shell_mode = Some(mode);
                self.input = command.to_string();
                self.paste_segments.clear();
                self.input_submission_mode = submission_mode;
                self.input_cursor = self.input_char_len();
                self.input_changed();
                return;
            }
        }
        self.shell_mode = None;
        self.input = text;
        self.paste_segments = paste_segments;
        self.input_submission_mode = submission_mode;
        self.input_cursor = self.input_char_len();
        self.input_changed();
    }

    pub(super) async fn finish_completed_inline_shells(&mut self) -> anyhow::Result<bool> {
        let mut changed = false;
        for task in &mut self.pending_inline_shells {
            changed |= task.drain_updates();
        }
        while self
            .pending_inline_shells
            .first()
            .is_some_and(|task| task.handle.is_finished())
        {
            let mut task = self.pending_inline_shells.remove(0);
            task.drain_updates();
            self.finish_inline_shell_task(task).await?;
            changed = true;
        }
        Ok(changed)
    }

    pub(super) async fn finish_all_inline_shells(&mut self) -> anyhow::Result<()> {
        while !self.pending_inline_shells.is_empty() {
            let task = self.pending_inline_shells.remove(0);
            self.finish_inline_shell_task(task).await?;
        }
        Ok(())
    }

    async fn finish_inline_shell_task(&mut self, task: PendingShellTask) -> anyhow::Result<()> {
        let output = match task.handle.await? {
            Ok(output) => output,
            Err(error) => {
                self.insert_entry(&super::Entry::Error(format!(
                    "could not run inline shell: {error}"
                )));
                self.status = "inline shell failed".into();
                return Ok(());
            }
        };
        if task.mode.included_in_context() {
            self.deferred_inline_shell_context
                .push(DeferredShellContext {
                    context: rho_tools::tool::truncate(
                        context_text(&output),
                        task.max_output_bytes,
                    ),
                    persisted_display: rho_tools::tool::truncate(
                        format!(
                            "!{}\n\n{}",
                            output.command,
                            display_text(&output, /*included_in_context*/ true)
                        ),
                        task.max_output_bytes,
                    ),
                });
        }
        let display_text = rho_tools::tool::truncate(
            display_text(&output, task.mode.included_in_context()),
            task.max_output_bytes,
        );
        self.finish_streams();
        self.insert_entry(&super::Entry::Tool(super::ToolEntry {
            state: super::ToolEntryState::Finished {
                ok: output.ok,
                display_style: rho_tools::tool::ToolDisplayStyle::file_or_command(),
            },
            display_lines: display_text.lines().map(str::to_string).collect(),
            expanded: true,
            image: None,
        }));
        self.statusline.refresh_git_branch();
        self.status = if output.ok {
            if task.mode.included_in_context() {
                "shell output pending context insertion".into()
            } else {
                "shell output excluded from context".into()
            }
        } else {
            format!("shell exited with {}", output.exit_code)
        };
        Ok(())
    }

    pub(super) fn running_inline_shell_entries(
        &self,
    ) -> impl Iterator<Item = super::ToolEntry> + '_ {
        self.pending_inline_shells
            .iter()
            .map(PendingShellTask::tool_entry)
    }

    pub(super) fn insert_deferred_inline_shell_context(
        &mut self,
        agent: &mut super::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let inserted = !self.deferred_inline_shell_context.is_empty();
        for deferred in std::mem::take(&mut self.deferred_inline_shell_context) {
            agent.append_user_context_with_display(deferred.context, deferred.persisted_display)?;
        }
        if inserted && !self.is_ui_busy() {
            self.status = "shell output included in context".into();
        }
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
        self.shell_mode = None;
        self.input_cursor = 0;
        self.clamp_command_selection();
    }
}

impl PendingShellTask {
    fn drain_updates(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.updates.try_recv() {
            match update.kind {
                ShellStreamKind::Stdout => self.stdout.push_str(&update.text),
                ShellStreamKind::Stderr => self.stderr.push_str(&update.text),
            }
            changed = true;
        }
        changed
    }

    fn tool_entry(&self) -> super::ToolEntry {
        let output = ShellOutput {
            shell: self.shell.clone(),
            command: self.command.clone(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            exit_code: "running".into(),
            ok: true,
        };
        super::ToolEntry {
            state: super::ToolEntryState::Running,
            display_lines: display_lines(&output, self.mode.included_in_context()),
            expanded: true,
            image: None,
        }
    }
}

#[cfg(test)]
#[path = "inline_shell_tests.rs"]
mod tests;
