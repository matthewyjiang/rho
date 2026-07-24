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
use super::windows_shim_args::{validate_cmd_args, validate_powershell_args, WindowsShimArgError};

#[cfg(test)]
use super::windows_shim_args::bat_command_line;

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

    /// Build a process command without pre-spawn validation.
    ///
    /// Auth/login use this for fixed short argv that never carries untrusted or
    /// multiline agent data. Production subagent spawns must use
    /// [`Self::try_command`] so Windows shim validation becomes a typed error.
    pub(crate) fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        build_command(self.kind, &self.program, args)
    }

    /// Fallible command build with typed pre-spawn validation for Windows shims.
    ///
    /// Production session spawns use this path so CR/LF/NUL (and other
    /// non-representable bat values) become [`ClaudeExecutableError`] before
    /// `CreateProcess`, not a generic I/O failure.
    pub(crate) fn try_command<I, S>(&self, args: I) -> Result<Command, ClaudeExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect_args(args);
        match self.kind {
            ClaudeInvocationKind::Direct => Ok(build_command(self.kind, &self.program, &args)),
            ClaudeInvocationKind::CmdScript => {
                validate_cmd_args(&self.program, &args)?;
                Ok(build_command(self.kind, &self.program, &args))
            }
            ClaudeInvocationKind::PowerShellScript => {
                validate_powershell_args(&args)?;
                Ok(build_command(self.kind, &self.program, &args))
            }
        }
    }

    /// Pure argv / command-line plan (tests and diagnostics).
    #[cfg(test)]
    pub(crate) fn try_argv<I, S>(&self, args: I) -> Result<ClaudeArgv, ClaudeExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect_args(args);
        argv_for(self.kind, &self.program, &args)
    }

    /// Infallible argv plan for tests that only use safe args.
    #[cfg(test)]
    pub(crate) fn argv<I, S>(&self, args: I) -> ClaudeArgv
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.try_argv(args)
            .expect("argv plan should succeed for safe test args")
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

fn build_command<I, S>(kind: ClaudeInvocationKind, program: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    match kind {
        ClaudeInvocationKind::Direct | ClaudeInvocationKind::CmdScript => {
            let mut command = Command::new(program);
            command.args(args);
            command
        }
        ClaudeInvocationKind::PowerShellScript => {
            let mut command = Command::new("powershell.exe");
            command.arg("-NoProfile");
            command.arg("-NonInteractive");
            command.arg("-ExecutionPolicy");
            command.arg("Bypass");
            command.arg("-File");
            command.arg(program);
            command.args(args);
            command
        }
    }
}

/// Pure process plan: program image plus argv, or a bat-encoded command line.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeArgv {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    /// When set, Windows bat encoding of the full command line (std-compatible).
    pub(crate) windows_command_line: Option<OsString>,
}

#[cfg(test)]
fn argv_for(
    kind: ClaudeInvocationKind,
    program: &Path,
    args: &[OsString],
) -> Result<ClaudeArgv, ClaudeExecutableError> {
    match kind {
        ClaudeInvocationKind::Direct => Ok(ClaudeArgv {
            program: program.to_path_buf(),
            args: args.to_vec(),
            windows_command_line: None,
        }),
        ClaudeInvocationKind::CmdScript => {
            let line = bat_command_line(program, args)?;
            Ok(ClaudeArgv {
                // Spawn image is the script; std rewrites to cmd.exe at spawn.
                program: program.to_path_buf(),
                args: args.to_vec(),
                windows_command_line: Some(line),
            })
        }
        ClaudeInvocationKind::PowerShellScript => {
            validate_powershell_args(args)?;
            let mut argv = vec![
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                program.as_os_str().to_os_string(),
            ];
            argv.extend(args.iter().cloned());
            Ok(ClaudeArgv {
                program: PathBuf::from("powershell.exe"),
                args: argv,
                windows_command_line: None,
            })
        }
    }
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
