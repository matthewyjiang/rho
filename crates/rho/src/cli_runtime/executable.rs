//! Resolve external CLI programs and build structured process commands.
//!
//! Windows may install external tools (Claude Code, Cursor CLI, etc.) as native
//! `.exe` binaries or as `.cmd` / `.ps1` shims (via npm, scoop, pip, etc.).
//! Bare `Command::new("tool")` misses those shims, and joining arguments through
//! a shell invites command injection.
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

use super::windows_shim_args::{bat_command_line, validate_powershell_args, WindowsShimArgError};

/// How to invoke a resolved external CLI binary or shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CliInvocationKind {
    /// Direct executable (Unix binary or Windows `.exe`).
    Direct,
    /// Windows `cmd` script shim (`.cmd` / `.bat`).
    CmdScript,
    /// Windows PowerShell script shim (`.ps1`).
    PowerShellScript,
}

/// Resolved path and invocation strategy for an external CLI program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CliExecutable {
    program: PathBuf,
    kind: CliInvocationKind,
}

/// Pre-spawn failures when args cannot be represented safely for a shim.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CliExecutableError {
    #[error(transparent)]
    WindowsShim(#[from] WindowsShimArgError),
}

impl CliExecutable {
    /// Build from an already-resolved path. Classifies by extension.
    pub(crate) fn from_path(path: impl Into<PathBuf>) -> Self {
        let program = path.into();
        let kind = classify_program(&program);
        Self { program, kind }
    }

    pub(crate) fn display(&self) -> String {
        crate::paths::display(&self.program)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.program
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> CliInvocationKind {
        self.kind
    }

    /// Resolve the exact process plan for `args`.
    ///
    /// This is the only place invocation rules live. CR/LF/NUL (and other
    /// values a bat wrapper cannot represent) become
    /// [`CliExecutableError`] here, before `CreateProcess`, rather than a
    /// generic I/O failure at spawn.
    pub(crate) fn plan<I, S>(&self, args: I) -> Result<CliArgv, CliExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect_args(args);
        match self.kind {
            CliInvocationKind::Direct => Ok(CliArgv {
                program: self.program.clone(),
                args,
            }),
            CliInvocationKind::CmdScript => {
                // Building the bat line is the same check std performs at spawn.
                // Validate the same values std refuses at spawn (CR/LF/NUL/...).
                let _ = bat_command_line(&self.program, &args)?;
                Ok(CliArgv {
                    // Spawn image is the script; std rewrites to cmd.exe.
                    program: self.program.clone(),
                    args,
                })
            }
            CliInvocationKind::PowerShellScript => {
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
                Ok(CliArgv {
                    program: PathBuf::from("powershell.exe"),
                    args: argv,
                })
            }
        }
    }

    /// Plan and build the process command in one step.
    pub(crate) fn try_command<I, S>(&self, args: I) -> Result<Command, CliExecutableError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.plan(args)?.command())
    }
}

/// Locate a program for spawning. On Windows, checks real binaries and then
/// `.cmd` / `.ps1` shims that Rust's bare-name lookup will not find.
pub(crate) fn resolve_named(program: &str) -> Option<CliExecutable> {
    if program.contains('/') || program.contains('\\') {
        let path = PathBuf::from(program);
        if path.is_file() {
            return Some(CliExecutable::from_path(path));
        }
        return None;
    }

    if let Some(path) = crate::executable::find_on_path(program) {
        return Some(CliExecutable::from_path(path));
    }

    #[cfg(windows)]
    {
        for candidate in [format!("{program}.cmd"), format!("{program}.ps1")] {
            if let Some(path) = crate::executable::find_on_path(&candidate) {
                return Some(CliExecutable::from_path(path));
            }
        }
    }

    None
}

fn classify_program(path: &Path) -> CliInvocationKind {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "cmd" | "bat" => CliInvocationKind::CmdScript,
        "ps1" => CliInvocationKind::PowerShellScript,
        _ => CliInvocationKind::Direct,
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

/// A validated process plan: image and argv.
///
/// For `.cmd` / `.bat` shims, `plan` rejects values std cannot encode; spawn still
/// uses `Command::args` so Rust's bat quoting is the source of truth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CliArgv {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
}

impl CliArgv {
    /// Build the process command. Invocation rules are already resolved, so
    /// every kind spawns the same way: image plus argv.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
