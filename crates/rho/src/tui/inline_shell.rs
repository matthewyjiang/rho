pub(super) use crate::config::default_inline_shell as default_shell;

use std::{path::Path, process::Stdio};

use rho_sdk::TRUNCATION_MARKER;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
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
    /// Rendered card lines reused across frames. A running shell's output only
    /// grows, so buffer lengths plus theme generation are a complete content
    /// key; without this every animation frame re-cloned and re-wrapped the
    /// whole captured output.
    render_cache: Option<ShellRenderCache>,
}

impl PendingShellTask {
    #[cfg(test)]
    pub(super) fn test_task(stdout: impl Into<String>) -> Self {
        let (_tx, rx) = mpsc::unbounded_channel();
        Self {
            mode: InlineShellMode::IncludeInContext,
            max_output_bytes: crate::config::DEFAULT_MAX_OUTPUT_BYTES,
            shell: "sh".into(),
            command: "printf hello".into(),
            stdout: stdout.into(),
            stderr: String::new(),
            updates: rx,
            handle: tokio::spawn(std::future::pending()),
            render_cache: None,
        }
    }
}

/// Keyed render for one running shell's card.
struct ShellRenderCache {
    stdout_len: usize,
    stderr_len: usize,
    width: usize,
    max_tool_output_lines: usize,
    max_image_height: u16,
    theme_generation: u64,
    lines: Vec<ratatui::text::Line<'static>>,
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
    execute_streaming(
        shell,
        command,
        cwd,
        None,
        crate::config::DEFAULT_MAX_OUTPUT_BYTES,
    )
    .await
}

async fn execute_streaming(
    shell: &str,
    command: &str,
    cwd: &Path,
    updates: Option<mpsc::UnboundedSender<ShellStreamUpdate>>,
    max_output_bytes: usize,
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
            // Login shells reset PATH in /etc/profile; carry the parent PATH across
            // so the inline shell matches the bash tool (see shell_process).
            process.args(["-lc", &rho_tools::login_shell_script(command)]);
            if let Some(path) = rho_tools::parent_path_for(&rho_sdk::ProcessEnvironment::InheritAll)
            {
                process.env(rho_tools::PARENT_PATH_VAR, path);
            }
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
    // One shared deadline for the readers and the wait. A command that leaves a
    // background process holding the pipe never reaches EOF, so readers that only
    // waited on EOF would outlive the killed child and hang the task forever.
    let deadline = tokio::time::Instant::now() + INLINE_SHELL_TIMEOUT;
    let stdout_updates = updates.clone();
    let stdout_reader = read_stream(
        stdout,
        ShellStreamKind::Stdout,
        stdout_updates,
        deadline,
        max_output_bytes,
    );
    let stderr_reader = read_stream(
        stderr,
        ShellStreamKind::Stderr,
        updates,
        deadline,
        max_output_bytes,
    );
    let wait = async {
        match tokio::time::timeout_at(deadline, child.wait()).await {
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

/// Marker appended when output is cut short.
const TRUNCATION_NOTICE: &str = TRUNCATION_MARKER;

/// Length of the trailing bytes that begin a UTF-8 sequence the read did not finish.
///
/// Decoding a raw read chunk would replace a character split across the chunk
/// boundary with U+FFFD, so those bytes stay buffered until the next read
/// completes them. A sequence is at most four bytes, so at most three can be
/// pending.
fn incomplete_utf8_suffix_len(bytes: &[u8]) -> usize {
    for back in 1..=3.min(bytes.len()) {
        let byte = bytes[bytes.len() - back];
        if byte < 0x80 {
            // ASCII never continues a sequence.
            return 0;
        }
        if byte >= 0xC0 {
            let needed = if byte >= 0xF0 {
                4
            } else if byte >= 0xE0 {
                3
            } else {
                2
            };
            return if back < needed { back } else { 0 };
        }
    }
    0
}

/// Reads a child pipe until EOF or `deadline`, keeping at most `max_output_bytes`.
///
/// Reading continues after the cap so the child never blocks on a full pipe; the
/// extra bytes are dropped instead of buffered.
async fn read_stream(
    mut stream: impl AsyncRead + Unpin,
    kind: ShellStreamKind,
    updates: Option<mpsc::UnboundedSender<ShellStreamUpdate>>,
    deadline: tokio::time::Instant,
    max_output_bytes: usize,
) -> std::io::Result<String> {
    let mut output = Vec::new();
    let mut undecoded = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 4096];
    loop {
        let read = match tokio::time::timeout_at(deadline, stream.read(&mut buffer)).await {
            Ok(read) => read?,
            // The deadline kills the child; return what arrived before it.
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        let free = max_output_bytes.saturating_sub(output.len());
        if free == 0 {
            truncated = true;
            continue;
        }
        let kept = free.min(chunk.len());
        truncated |= kept < chunk.len();
        output.extend_from_slice(&chunk[..kept]);
        if let Some(updates) = &updates {
            undecoded.extend_from_slice(&chunk[..kept]);
            let complete = undecoded.len() - incomplete_utf8_suffix_len(&undecoded);
            if complete > 0 {
                let text = String::from_utf8_lossy(&undecoded[..complete]).into_owned();
                undecoded.drain(..complete);
                let _ = updates.send(ShellStreamUpdate { kind, text });
            }
        }
    }
    if let Some(updates) = &updates {
        let mut tail = String::from_utf8_lossy(&undecoded).into_owned();
        if truncated {
            tail.push_str(TRUNCATION_NOTICE);
        }
        if !tail.is_empty() {
            let _ = updates.send(ShellStreamUpdate { kind, text: tail });
        }
    }
    let mut text = String::from_utf8_lossy(&output).into_owned();
    if truncated {
        text.push_str(TRUNCATION_NOTICE);
    }
    Ok(text)
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
    let card = display_card(output, included_in_context);
    let mut parts = vec![card.header_text()];
    for fact in &card.facts {
        parts.push(fact.plain_text());
    }
    let body = card.body.plain_lines();
    if !body.is_empty() {
        if !card.facts.is_empty() {
            parts.push(String::new());
        }
        parts.extend(body);
    }
    parts.join("\n")
}

pub(super) fn display_card(output: &ShellOutput, _included_in_context: bool) -> ToolCard {
    ShellCardParts::from_output(output).card()
}

/// The borrowed inputs one shell card renders from.
///
/// Live tasks render straight from their streaming buffers and finished runs
/// from their [`ShellOutput`], without cloning either into the other's shape.
struct ShellCardParts<'a> {
    shell: &'a str,
    command: &'a str,
    stdout: &'a str,
    stderr: &'a str,
    exit_code: &'a str,
    ok: bool,
}

impl<'a> ShellCardParts<'a> {
    fn from_output(output: &'a ShellOutput) -> Self {
        Self {
            shell: &output.shell,
            command: &output.command,
            stdout: &output.stdout,
            stderr: &output.stderr,
            exit_code: &output.exit_code,
            ok: output.ok,
        }
    }

    /// A still-running task has no exit code yet and owns no failure.
    fn running(task: &'a PendingShellTask) -> Self {
        Self {
            shell: &task.shell,
            command: &task.command,
            stdout: &task.stdout,
            stderr: &task.stderr,
            exit_code: "running",
            ok: true,
        }
    }

    fn card(self) -> ToolCard {
        let prompt = match self.shell.to_ascii_lowercase().as_str() {
            "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => "PS",
            _ => "$",
        };
        let status = if self.exit_code == "running" {
            ToolStatus::Running
        } else if self.ok {
            ToolStatus::Ok
        } else if self.exit_code == "cancelled" {
            ToolStatus::Interrupted
        } else {
            ToolStatus::Error
        };
        let mut card = ToolCard::new(
            status,
            ToolFamily::FileCommand,
            ToolHeader::shell(prompt, Some(self.command.to_string())),
        );
        if self.exit_code != "running" && !self.ok && self.exit_code != "cancelled" {
            card.push_fact(ToolFact::Meta {
                text: format!("exit {}", self.exit_code),
            });
        }
        if !self.stdout.is_empty() {
            card.body = ToolBody::Lines(vec![self.stdout.trim_end().to_string()]);
        } else if !self.stderr.is_empty() && !self.ok {
            card.push_fact(ToolFact::Error {
                text: self.stderr.trim_end().to_string(),
            });
        }
        card
    }
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
            self.set_status("enter a shell command after ! or !!");
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
        let max_output_bytes = config.max_output_bytes;
        let (updates_tx, updates_rx) = mpsc::unbounded_channel();
        self.pending_inline_shells.push(PendingShellTask {
            mode,
            max_output_bytes,
            shell: shell.clone(),
            command: command.clone(),
            stdout: String::new(),
            stderr: String::new(),
            updates: updates_rx,
            handle: tokio::spawn(async move {
                execute_streaming(
                    &task_shell,
                    &task_command,
                    &cwd,
                    Some(updates_tx),
                    max_output_bytes,
                )
                .await
            }),
            render_cache: None,
        });
        self.set_status(format!("running {shell}"));
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
            self.insert_entry(&super::Entry::Tool(super::ToolEntry::new(
                display_card(&output, task.mode.included_in_context()),
                true,
                None,
                None,
            )));
        }
        self.set_status("inline shell cancelled");
        true
    }

    /// Leave shell mode and restore a normal composer, keeping the typed command text.
    pub(super) fn exit_shell_mode(&mut self) -> bool {
        if self.input_ui.take_shell_mode().is_none() {
            return false;
        }
        self.set_status(self.busy_status_label());
        true
    }

    /// Enter or upgrade shell mode from a leading `!` keypress.
    ///
    /// Shell mode is explicit App state. The composer stores only the command
    /// text, so cursor/home/delete/word/paste coordinates stay ordinary.
    pub(super) fn try_enter_shell_mode_from_bang(&mut self) -> bool {
        if !matches!(self.input_ui.composer(), super::ComposerMode::Input)
            || self.input_ui.cursor() != 0
            || !self.input_ui.text().is_empty()
            || !self.input_ui.paste_segments().is_empty()
        {
            return false;
        }
        match self.input_ui.shell_mode() {
            None => {
                *self.input_ui.shell_mode_mut() = Some(InlineShellMode::IncludeInContext);
                true
            }
            Some(InlineShellMode::IncludeInContext) => {
                *self.input_ui.shell_mode_mut() = Some(InlineShellMode::ExcludeFromContext);
                true
            }
            Some(InlineShellMode::ExcludeFromContext) => true,
        }
    }

    pub(super) fn shell_submission(&self) -> Option<(InlineShellMode, String)> {
        if let Some(mode) = self.input_ui.shell_mode() {
            return Some((mode, self.input_ui.text().trim().to_string()));
        }
        InlineShellMode::parse(self.input_ui.text().trim())
            .map(|(mode, command)| (mode, command.to_string()))
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
                *self.input_ui.shell_mode_mut() = Some(mode);
                self.input_ui.set_text(command.to_string());
                self.input_ui.clear_paste_segments();
                self.input_ui.set_submission_mode(submission_mode);
                self.input_ui.set_cursor(self.input_char_len());
                self.clamp_command_selection();
                self.clamp_file_selection();
                return;
            }
        }
        self.input_ui.set_shell_mode(None);
        self.input_ui.set_text(text);
        self.input_ui.set_paste_segments(paste_segments);
        self.input_ui.set_submission_mode(submission_mode);
        self.input_ui.set_cursor(self.input_char_len());
        self.clamp_command_selection();
        self.clamp_file_selection();
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
                self.set_status("inline shell failed");
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
        self.finish_streams();
        self.insert_entry(&super::Entry::Tool(super::ToolEntry::new(
            display_card(&output, task.mode.included_in_context()),
            true,
            None,
            None,
        )));
        self.refresh_git_after_command(Some(output.command.as_str()));
        self.set_status(if output.ok {
            if task.mode.included_in_context() {
                "shell output pending context insertion".to_string()
            } else {
                "shell output excluded from context".to_string()
            }
        } else {
            format!("shell exited with {}", output.exit_code)
        });
        Ok(())
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
            self.set_status("shell output included in context");
        }
        Ok(())
    }

    pub(super) fn block_pasted_inline_shell(&mut self) -> anyhow::Result<()> {
        self.insert_entry(&super::Entry::Error(
            "inline shell commands cannot start from collapsed pasted content".into(),
        ));
        self.set_status("inline shell paste blocked");
        Ok(())
    }

    pub(super) fn clear_submitted_input(&mut self) {
        self.cancel_all_pending_attachments();
        self.input_ui.clear_submitted();
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
        super::ToolEntry::new(ShellCardParts::running(self).card(), true, None, None)
    }

    /// Cached render of this task's live card, refreshed only when the output
    /// buffers, width, budgets, or theme changed since the last frame.
    pub(super) fn rendered_lines(
        &mut self,
        width: usize,
        max_tool_output_lines: usize,
        max_image_height: u16,
    ) -> &[ratatui::text::Line<'static>] {
        let theme_generation = super::Theme::generation();
        let fresh = self.render_cache.as_ref().is_some_and(|cache| {
            cache.stdout_len == self.stdout.len()
                && cache.stderr_len == self.stderr.len()
                && cache.width == width
                && cache.max_tool_output_lines == max_tool_output_lines
                && cache.max_image_height == max_image_height
                && cache.theme_generation == theme_generation
        });
        if !fresh {
            let lines = super::tool_entry_lines(
                &self.tool_entry(),
                width,
                max_tool_output_lines,
                max_image_height,
            );
            self.render_cache = Some(ShellRenderCache {
                stdout_len: self.stdout.len(),
                stderr_len: self.stderr.len(),
                width,
                max_tool_output_lines,
                max_image_height,
                theme_generation,
                lines,
            });
        }
        &self
            .render_cache
            .as_ref()
            .expect("render cache populated above")
            .lines
    }
}

#[cfg(test)]
#[path = "inline_shell_tests.rs"]
mod tests;
