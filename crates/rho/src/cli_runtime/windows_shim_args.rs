//! Windows shim argument encoding for external CLI `.cmd` / `.bat` / `.ps1` wrappers.
//!
//! ## `.cmd` / `.bat`
//!
//! `CreateProcess` cannot run batch files as native images. Spawning a `.cmd`
//! path goes through `cmd.exe`, which reparses the command line with its own
//! rules. Passing separate Rust argv tokens into an explicit `cmd.exe /C`
//! invocation is not safe: metacharacters such as `& | ^ ( )` and quote toggles
//! can break out of the intended argument (BatBadBut / CVE-2024-24576 class).
//!
//! Production spawns use `Command::new(script).args(args)` so Rust `std` applies
//! `make_bat_command_line` at spawn time. The pure encoder below mirrors that
//! algorithm (`library/std/src/sys/args/windows.rs`, `append_bat_arg`) for tests
//! and for pre-spawn rejection of values std also refuses (CR / LF / NUL):
//! - outer wrapper: `cmd.exe /e:ON /v:OFF /d /c " ... "` (`/d` skips AutoRun,
//!   `/v:OFF` disables `!delayed!` expansion, `/e:ON` enables the `%` null-slice
//!   distraction);
//! - quote when empty, trailing `\`, ASCII punctuation outside a small safe set,
//!   or a control character;
//! - inner `"` doubled as `""`; trailing `\` before a closing quote doubled;
//! - `%` rewritten as `%%cd:~,%` + `%` so `cmd` does not expand `%VAR%`
//!   (yt-dlp / Rust std zero-length `%cd:~,%` slice hack);
//! - CR and LF rejected (they truncate the line).
//!
//! ## `.ps1`
//!
//! Launch as `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass
//! -File <script> <args...>` with ordinary structured argv. `-File` passes
//! trailing tokens as literal script arguments (no `-Command` string). NUL is
//! rejected; other values stay structured.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Pre-spawn failure when args cannot be forwarded safely through a Windows shim.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WindowsShimArgError {
    #[error(
        "argument cannot be passed safely through a Windows cmd/bat shim \
(contains CR, LF, or NUL)"
    )]
    CmdDisallowedByte,
    #[error(
        "Windows cmd/bat shim path is invalid \
(contains quote or ends with backslash)"
    )]
    InvalidScriptPath,
    #[error(
        "argument cannot be passed safely through a Windows PowerShell shim \
(contains NUL)"
    )]
    PowerShellDisallowedByte,
}

/// Full `lpCommandLine` including the `cmd.exe` image name, matching Rust std's
/// `make_bat_command_line` encoding.
///
/// Building this line is also how cmd/bat args are validated: anything std
/// would refuse at spawn fails here instead.
pub(crate) fn bat_command_line(
    script: &Path,
    args: &[impl AsRef<OsStr>],
) -> Result<OsString, WindowsShimArgError> {
    let mut line = OsString::from("cmd.exe ");
    line.push(bat_raw_arg_tail(script, args)?);
    Ok(line)
}

/// Tail after `cmd.exe` for callers that set the program image separately.
pub(crate) fn bat_raw_arg_tail(
    script: &Path,
    args: &[impl AsRef<OsStr>],
) -> Result<OsString, WindowsShimArgError> {
    let script_os = script.as_os_str();
    validate_script_path(script_os)?;

    // Matches std: cmd.exe /e:ON /v:OFF /d /c " <script> <args...> "
    let mut line = String::from("/e:ON /v:OFF /d /c \"");
    line.push('"');
    push_os_str(&mut line, script_os)?;
    line.push('"');

    for arg in args {
        line.push(' ');
        append_bat_arg_str(&mut line, arg.as_ref())?;
    }
    line.push('"');
    Ok(OsString::from(line))
}

/// Validate PowerShell `-File` trailing args (structured argv; only NUL is fatal).
pub(crate) fn validate_powershell_args(
    args: &[impl AsRef<OsStr>],
) -> Result<(), WindowsShimArgError> {
    for arg in args {
        if os_contains_nul(arg.as_ref()) {
            return Err(WindowsShimArgError::PowerShellDisallowedByte);
        }
    }
    Ok(())
}

fn validate_script_path(script: &OsStr) -> Result<(), WindowsShimArgError> {
    let bytes = script.as_encoded_bytes();
    if bytes.contains(&0) {
        return Err(WindowsShimArgError::CmdDisallowedByte);
    }
    if bytes.contains(&b'"') || bytes.last() == Some(&b'\\') {
        return Err(WindowsShimArgError::InvalidScriptPath);
    }
    Ok(())
}

/// Quote/escape one bat argument into `out` (std `append_bat_arg` algorithm).
fn append_bat_arg_str(out: &mut String, arg: &OsStr) -> Result<(), WindowsShimArgError> {
    if os_contains_nul(arg) {
        return Err(WindowsShimArgError::CmdDisallowedByte);
    }
    let Some(text) = arg.to_str() else {
        // Non-unicode args are not used by external CLI spawns; refuse rather than lossy-send.
        return Err(WindowsShimArgError::CmdDisallowedByte);
    };
    if text.chars().any(|c| c == '\r' || c == '\n') {
        return Err(WindowsShimArgError::CmdDisallowedByte);
    }

    let mut quote = text.is_empty() || text.as_bytes().last() == Some(&b'\\');
    static UNQUOTED: &str = r"#$*+-./:?@\_";
    for cp in text.chars() {
        let ascii_needs_quotes =
            cp.is_ascii() && !(cp.is_ascii_alphanumeric() || UNQUOTED.contains(cp));
        if ascii_needs_quotes || cp.is_control() {
            quote = true;
            break;
        }
    }

    if quote {
        out.push('"');
    }

    // std append_bat_arg: count runs of `\`; always emit the current char.
    // Before `"`, emit n extra `\` (total 2n) and one extra `"` (doubling).
    // Before ending `"`, emit n extra `\` (total 2n).
    // Before `%`/`\r`, emit the `%%cd:~,` distraction, then the original char.
    let mut backslashes = 0usize;
    for ch in text.chars() {
        if ch == '\\' {
            backslashes += 1;
        } else {
            if ch == '"' {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                // Doubled quote is both cmd and CRT literal-quote form.
                out.push('"');
            } else if ch == '%' || ch == '\r' {
                // yt-dlp / Rust std: zero-length %cd:~,% slice distracts %VAR%.
                out.push_str("%%cd:~,");
            }
            backslashes = 0;
        }
        out.push(ch);
    }

    if quote {
        for _ in 0..backslashes {
            out.push('\\');
        }
        out.push('"');
    }
    Ok(())
}

fn os_contains_nul(arg: &OsStr) -> bool {
    arg.as_encoded_bytes().contains(&0)
}

fn push_os_str(out: &mut String, arg: &OsStr) -> Result<(), WindowsShimArgError> {
    let Some(text) = arg.to_str() else {
        return Err(WindowsShimArgError::InvalidScriptPath);
    };
    out.push_str(text);
    Ok(())
}

#[cfg(test)]
#[path = "windows_shim_args_tests.rs"]
mod tests;
