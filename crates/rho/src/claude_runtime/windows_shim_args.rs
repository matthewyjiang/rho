//! Windows shim argument encoding for Claude Code `.cmd` / `.bat` / `.ps1`.
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
        "claude code: argument cannot be passed safely through a Windows cmd/bat shim \
(contains CR, LF, or NUL)"
    )]
    CmdDisallowedByte,
    #[error(
        "claude code: Windows cmd/bat shim path is invalid \
(contains quote or ends with backslash)"
    )]
    InvalidScriptPath,
    #[error(
        "claude code: argument cannot be passed safely through a Windows PowerShell shim \
(contains NUL)"
    )]
    PowerShellDisallowedByte,
}

/// Full `lpCommandLine` including the `cmd.exe` image name, matching Rust std's
/// `make_bat_command_line` encoding (for pure tests and diagnostics).
#[cfg(test)]
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

/// Reject cmd/bat args that std also refuses before spawn.
pub(crate) fn validate_cmd_args(
    script: &Path,
    args: &[impl AsRef<OsStr>],
) -> Result<(), WindowsShimArgError> {
    bat_raw_arg_tail(script, args).map(|_| ())
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
        // Non-unicode args are not used by Claude spawns; refuse rather than lossy-send.
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
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn line(script: &str, args: &[&str]) -> String {
        bat_command_line(Path::new(script), args)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn tail(script: &str, args: &[&str]) -> String {
        bat_raw_arg_tail(Path::new(script), args)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn bat_line_wraps_script_and_simple_args() {
        assert_eq!(
            tail(r"C:\shims\claude.cmd", &["auth", "status"]),
            r#"/e:ON /v:OFF /d /c ""C:\shims\claude.cmd" auth status""#
        );
        assert!(line(r"C:\shims\claude.cmd", &["auth"]).starts_with("cmd.exe "));
    }

    #[test]
    fn bat_line_quotes_spaces_and_empty() {
        assert_eq!(
            tail(r"C:\shims\claude.cmd", &["--system", "a b", ""]),
            r#"/e:ON /v:OFF /d /c ""C:\shims\claude.cmd" --system "a b" """"#
        );
    }

    #[test]
    fn bat_line_quotes_metacharacters() {
        let got = tail(
            r"C:\shims\claude.cmd",
            &["a&b", "c|d", "e(f)", "x^y", "p>q", "r<s"],
        );
        assert!(got.contains(r#""a&b""#), "{got}");
        assert!(got.contains(r#""c|d""#), "{got}");
        assert!(got.contains(r#""e(f)""#), "{got}");
        assert!(got.contains(r#""x^y""#), "{got}");
        assert!(got.contains(r#""p>q""#), "{got}");
        assert!(got.contains(r#""r<s""#), "{got}");
    }

    #[test]
    fn bat_line_doubles_embedded_quotes() {
        let got = tail(r"C:\shims\claude.cmd", &[r#"say "hi""#]);
        assert!(got.contains(r#""say ""hi""""#), "{got}");
    }

    #[test]
    fn bat_line_percent_uses_std_null_slice_hack() {
        let got = tail(r"C:\shims\claude.cmd", &["100%sure", "%PATH%"]);
        // Each `%` becomes `%%cd:~,` + `%` (std yt-dlp null-slice of %cd%).
        assert!(got.contains("100%%cd:~,%sure"), "{got}");
        assert!(got.contains("%%cd:~,%PATH%%cd:~,%"), "{got}");
    }

    #[test]
    fn bat_line_exclamation_is_quoted_under_v_off() {
        let got = tail(r"C:\shims\claude.cmd", &["wow!"]);
        assert!(got.contains(r#""wow!""#), "{got}");
        assert!(got.contains("/v:OFF"), "{got}");
    }

    #[test]
    fn bat_line_unicode_passthrough() {
        let got = tail(r"C:\shims\claude.cmd", &["模型", "café"]);
        assert!(got.contains("模型"), "{got}");
        assert!(got.contains("café"), "{got}");
    }

    #[test]
    fn bat_line_rejects_cr_lf() {
        assert_eq!(
            bat_raw_arg_tail(Path::new(r"C:\claude.cmd"), &["ok\nbad"]).unwrap_err(),
            WindowsShimArgError::CmdDisallowedByte
        );
        assert_eq!(
            bat_raw_arg_tail(Path::new(r"C:\claude.cmd"), &["ok\rbad"]).unwrap_err(),
            WindowsShimArgError::CmdDisallowedByte
        );
    }

    #[test]
    fn bat_line_rejects_bad_script_path() {
        assert_eq!(
            bat_raw_arg_tail(Path::new(r#"C:\bad"name.cmd"#), &["x"]).unwrap_err(),
            WindowsShimArgError::InvalidScriptPath
        );
        assert_eq!(
            bat_raw_arg_tail(Path::new(r"C:\dir\"), &["x"]).unwrap_err(),
            WindowsShimArgError::InvalidScriptPath
        );
    }

    #[test]
    fn powershell_rejects_nul_only() {
        validate_powershell_args(&["ok", "a b", "%PATH%", "a&b"]).unwrap();
        let nul = OsString::from("a\0b");
        assert_eq!(
            validate_powershell_args(&[nul.as_os_str()]).unwrap_err(),
            WindowsShimArgError::PowerShellDisallowedByte
        );
    }

    #[test]
    fn trailing_backslash_argument_is_quoted_and_doubled() {
        let got = tail(r"C:\shims\claude.cmd", &[r"C:\path\"]);
        assert!(got.contains(r#""C:\path\\""#), "{got}");
    }

    #[test]
    fn script_path_buf_round_trip_in_line() {
        let script = PathBuf::from(r"C:\Users\me\scoop\shims\claude.cmd");
        let got = bat_raw_arg_tail(&script, &["logout"])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            got.contains(r#""C:\Users\me\scoop\shims\claude.cmd""#),
            "{got}"
        );
    }
}
