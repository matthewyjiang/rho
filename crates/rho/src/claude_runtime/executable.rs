//! Resolve the `claude` program and build fixed-argv process commands.
//!
//! Windows may install Claude Code as a real `.exe` or as a `.cmd` / `.ps1`
//! shim. Bare `Command::new("claude")` misses those shims, and shelling out
//! through a single joined command string invites injection.
//!
//! Invocation rules:
//! - **Direct** (Unix binary / Windows `.exe`): structured `Command::new(path)`
//!   plus args.
//! - **`.cmd` / `.bat`**: `Command::new(script).args(args)` so Rust `std` applies
//!   its bat-safe `make_bat_command_line` encoding at spawn (BatBadBut /
//!   CVE-2024-24576). Do **not** pass separate argv tokens through
//!   `cmd.exe /C` — `cmd` reparses that line. See [`super::windows_shim_args`].
//! - **`.ps1`**: `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy
//!   Bypass -File <script> <args...>` with structured argv.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::auth::{ClaudeAuthError, CLAUDE_PROGRAM};
use super::windows_shim_args::{bat_command_line, validate_powershell_args, WindowsShimArgError};

/// How to invoke a resolved Claude Code binary or shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaudeInvocationKind {
    /// Direct executable (Unix binary or Windows `.exe`).
    Direct,
    /// Windows `cmd` script shim (`.cmd` / `.bat`).
    CmdScript,
    /// Windows PowerShell script shim (`.ps1`).
    PowerShellScript,
}

/// Resolved path and invocation strategy for Claude Code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeExecutable {
    program: PathBuf,
    kind: ClaudeInvocationKind,
}

/// Pre-spawn failures when args cannot be represented safely for a shim.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClaudeExecutableError {
    #[error(transparent)]
    WindowsShim(#[from] WindowsShimArgError),
}

impl ClaudeExecutable {
    /// Build from an already-resolved path. Classifies by extension.
    pub(crate) fn from_path(path: impl Into<PathBuf>) -> Self {
        let program = path.into();
        let kind = classify_program(&program);
        Self { program, kind }
    }

    pub(crate) fn display(&self) -> String {
        crate::paths::display(&self.program)
    }

    #[cfg(test)]
    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> ClaudeInvocationKind {
        self.kind
    }

    /// Resolve the exact process plan for `args`.
    ///
    /// This is the only place invocation rules live. CR/LF/NUL (and other
    /// values a bat wrapper cannot represent) become
    /// [`ClaudeExecutableError`] here, before `CreateProcess`, rather than a
    /// generic I/O failure at spawn.
    pub(crate) fn plan<I, S>(&self, args: I) -> Result<ClaudeArgv, ClaudeExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect_args(args);
        match self.kind {
            ClaudeInvocationKind::Direct => Ok(ClaudeArgv {
                program: self.program.clone(),
                args,
                windows_command_line: None,
            }),
            ClaudeInvocationKind::CmdScript => {
                // Building the bat line is the same check std performs at spawn.
                let line = bat_command_line(&self.program, &args)?;
                Ok(ClaudeArgv {
                    // Spawn image is the script; std rewrites to cmd.exe.
                    program: self.program.clone(),
                    args,
                    windows_command_line: Some(line),
                })
            }
            ClaudeInvocationKind::PowerShellScript => {
                validate_powershell_args(&args)?;
                let mut argv = vec![
                    OsString::from("-NoProfile"),
                    OsString::from("-NonInteractive"),
                    OsString::from("-ExecutionPolicy"),
                    OsString::from("Bypass"),
                    OsString::from("-File"),
                    self.program.as_os_str().to_os_string(),
                ];
                argv.extend(args);
                Ok(ClaudeArgv {
                    program: PathBuf::from("powershell.exe"),
                    args: argv,
                    windows_command_line: None,
                })
            }
        }
    }

    /// Plan and build the process command in one step.
    pub(crate) fn try_command<I, S>(&self, args: I) -> Result<Command, ClaudeExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.plan(args)?.command())
    }
}

/// Locate `claude` for spawning. On Windows, prefer real binaries and then
/// `.cmd` / `.ps1` shims that Rust's bare-name lookup will not find.
pub(crate) fn resolve() -> Result<ClaudeExecutable, ClaudeAuthError> {
    resolve_named(CLAUDE_PROGRAM)
}

pub(crate) fn resolve_named(program: &str) -> Result<ClaudeExecutable, ClaudeAuthError> {
    if program.contains('/') || program.contains('\\') {
        let path = PathBuf::from(program);
        if path.is_file() {
            return Ok(ClaudeExecutable::from_path(path));
        }
        return Err(ClaudeAuthError::BinaryMissing);
    }

    if let Some(path) = crate::executable::find_on_path(program) {
        return Ok(ClaudeExecutable::from_path(path));
    }

    #[cfg(windows)]
    {
        for candidate in [format!("{program}.cmd"), format!("{program}.ps1")] {
            if let Some(path) = crate::executable::find_on_path(&candidate) {
                return Ok(ClaudeExecutable::from_path(path));
            }
        }
    }

    Err(ClaudeAuthError::BinaryMissing)
}

fn classify_program(path: &Path) -> ClaudeInvocationKind {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "cmd" | "bat" => ClaudeInvocationKind::CmdScript,
        "ps1" => ClaudeInvocationKind::PowerShellScript,
        _ => ClaudeInvocationKind::Direct,
    }
}

fn collect_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect()
}

/// A validated process plan: image, argv, and the bat command line std will
/// build for `.cmd` / `.bat` shims.
///
/// Production spawns and tests read the same value, so an argv assertion is an
/// assertion about the process that actually runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeArgv {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    /// Set for cmd shims: std-compatible bat encoding of the full command line.
    pub(crate) windows_command_line: Option<OsString>,
}

impl ClaudeArgv {
    /// Build the process command. Invocation rules are already resolved, so
    /// every kind spawns the same way: image plus argv.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
