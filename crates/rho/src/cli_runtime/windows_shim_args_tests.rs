use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;

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

// Covers: batch command line wrapper quotes script image and arguments.
// Owner: pure unit
#[test]
fn bat_line_wraps_script_and_simple_args() {
    assert_eq!(
        tail(r"C:\shims\tool.cmd", &["run", "status"]),
        r#"/e:ON /v:OFF /d /c ""C:\shims\tool.cmd" run status""#
    );
    assert!(line(r"C:\shims\tool.cmd", &["run"]).starts_with("cmd.exe "));
}

// Covers: space and empty arguments receive quotes.
// Owner: pure unit
#[test]
fn bat_line_quotes_spaces_and_empty() {
    assert_eq!(
        tail(r"C:\shims\tool.cmd", &["--system", "a b", ""]),
        r#"/e:ON /v:OFF /d /c ""C:\shims\tool.cmd" --system "a b" """"#
    );
}

// Covers: cmd metacharacters are safely quoted against injection.
// Owner: pure unit
#[test]
fn bat_line_quotes_metacharacters() {
    let got = tail(
        r"C:\shims\tool.cmd",
        &["a&b", "c|d", "e(f)", "x^y", "p>q", "r<s"],
    );
    assert!(got.contains(r#""a&b""#), "{got}");
    assert!(got.contains(r#""c|d""#), "{got}");
    assert!(got.contains(r#""e(f)""#), "{got}");
    assert!(got.contains(r#""x^y""#), "{got}");
    assert!(got.contains(r#""p>q""#), "{got}");
    assert!(got.contains(r#""r<s""#), "{got}");
}

// Covers: embedded quotes are doubled matching Windows CRT escaping.
// Owner: pure unit
#[test]
fn bat_line_doubles_embedded_quotes() {
    let got = tail(r"C:\shims\tool.cmd", &[r#"say "hi""#]);
    assert!(got.contains(r#""say ""hi""""#), "{got}");
}

// Covers: percent signs use the zero-length %cd:~,% slice hack to avoid environment expansion.
// Owner: pure unit
#[test]
fn bat_line_percent_uses_std_null_slice_hack() {
    let got = tail(r"C:\shims\tool.cmd", &["100%sure", "%PATH%"]);
    // Each `%` becomes `%%cd:~,` + `%` (std yt-dlp null-slice of %cd%).
    assert!(got.contains("100%%cd:~,%sure"), "{got}");
    assert!(got.contains("%%cd:~,%PATH%%cd:~,%"), "{got}");
}

// Covers: exclamation marks are quoted under /v:OFF.
// Owner: pure unit
#[test]
fn bat_line_exclamation_is_quoted_under_v_off() {
    let got = tail(r"C:\shims\tool.cmd", &["wow!"]);
    assert!(got.contains(r#""wow!""#), "{got}");
    assert!(got.contains("/v:OFF"), "{got}");
}

// Covers: unicode characters pass through verbatim.
// Owner: pure unit
#[test]
fn bat_line_unicode_passthrough() {
    let got = tail(r"C:\shims\tool.cmd", &["模型", "café"]);
    assert!(got.contains("模型"), "{got}");
    assert!(got.contains("café"), "{got}");
}

// Covers: CR and LF in arguments are rejected before spawn.
// Owner: pure unit
#[test]
fn bat_line_rejects_cr_lf() {
    assert_eq!(
        bat_raw_arg_tail(Path::new(r"C:\tool.cmd"), &["ok\nbad"]).unwrap_err(),
        WindowsShimArgError::CmdDisallowedByte
    );
    assert_eq!(
        bat_raw_arg_tail(Path::new(r"C:\tool.cmd"), &["ok\rbad"]).unwrap_err(),
        WindowsShimArgError::CmdDisallowedByte
    );
}

// Covers: script paths containing quotes or trailing backslashes are rejected.
// Owner: pure unit
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

// Covers: powershell arguments reject NUL but allow metacharacters.
// Owner: pure unit
#[test]
fn powershell_rejects_nul_only() {
    validate_powershell_args(&["ok", "a b", "%PATH%", "a&b"]).unwrap();
    let nul = OsString::from("a\0b");
    assert_eq!(
        validate_powershell_args(&[nul.as_os_str()]).unwrap_err(),
        WindowsShimArgError::PowerShellDisallowedByte
    );
}

// Covers: trailing backslash argument is quoted and doubled.
// Owner: pure unit
#[test]
fn trailing_backslash_argument_is_quoted_and_doubled() {
    let got = tail(r"C:\shims\tool.cmd", &[r"C:\path\"]);
    assert!(got.contains(r#""C:\path\\""#), "{got}");
}

// Covers: PathBuf conversion roundtrips in bat_raw_arg_tail.
// Owner: pure unit
#[test]
fn script_path_buf_round_trip_in_line() {
    let script = PathBuf::from(r"C:\Users\me\scoop\shims\tool.cmd");
    let got = bat_raw_arg_tail(&script, &["logout"])
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        got.contains(r#""C:\Users\me\scoop\shims\tool.cmd""#),
        "{got}"
    );
}
