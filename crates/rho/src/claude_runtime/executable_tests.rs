use std::ffi::OsString;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::claude_runtime::windows_shim_args::WindowsShimArgError;

#[test]
fn missing_bare_name_is_binary_missing() {
    let error = resolve_named("definitely-not-a-claude-binary-xyz").unwrap_err();
    assert!(error.is_binary_missing());
}

#[test]
fn absolute_missing_path_is_binary_missing() {
    let error = resolve_named("/tmp/rho-missing-claude-binary").unwrap_err();
    assert!(error.is_binary_missing());
}

#[test]
fn classifies_direct_unix_or_exe_paths() {
    let unix = ClaudeExecutable::from_path("/usr/bin/claude");
    assert_eq!(unix.kind(), ClaudeInvocationKind::Direct);
    assert_eq!(
        unix.plan(["auth", "status"]).unwrap(),
        ClaudeArgv {
            program: PathBuf::from("/usr/bin/claude"),
            args: vec![OsString::from("auth"), OsString::from("status")],
        }
    );

    let exe = ClaudeExecutable::from_path(r"C:\Tools\claude.exe");
    assert_eq!(exe.kind(), ClaudeInvocationKind::Direct);
    assert_eq!(
        exe.plan(["--version"]).unwrap(),
        ClaudeArgv {
            program: PathBuf::from(r"C:\Tools\claude.exe"),
            args: vec![OsString::from("--version")],
        }
    );
}

#[test]
fn classifies_cmd_shim_uses_script_image_and_bat_command_line() {
    let shim = ClaudeExecutable::from_path(r"C:\Users\me\scoop\shims\claude.cmd");
    assert_eq!(shim.kind(), ClaudeInvocationKind::CmdScript);
    let plan = shim.plan(["auth", "logout"]).unwrap();
    assert_eq!(
        plan.program,
        PathBuf::from(r"C:\Users\me\scoop\shims\claude.cmd")
    );
    assert_eq!(
        plan.args,
        vec![OsString::from("auth"), OsString::from("logout")]
    );
    let line =
        crate::claude_runtime::windows_shim_args::bat_command_line(&plan.program, &plan.args)
            .expect("cmd shim encodes bat command line")
            .to_string_lossy()
            .into_owned();
    // std-compatible wrapper: not bare `cmd /C` + separate unquoted argv.
    assert!(line.starts_with("cmd.exe /e:ON /v:OFF /d /c "), "{line}");
    assert!(
        line.contains(r#""C:\Users\me\scoop\shims\claude.cmd""#),
        "{line}"
    );
    assert!(line.contains("auth"), "{line}");
    assert!(line.contains("logout"), "{line}");
    // Must not be the old unsafe multi-arg form that cmd re-parses loosely.
    assert!(!line.contains("cmd.exe /D /C"), "{line}");
}

#[test]
fn classifies_ps1_shim_as_fixed_argv_powershell_invocation() {
    let shim = ClaudeExecutable::from_path(r"C:\Tools\claude.ps1");
    assert_eq!(shim.kind(), ClaudeInvocationKind::PowerShellScript);
    assert_eq!(
        shim.plan(["auth", "status"]).unwrap(),
        ClaudeArgv {
            program: PathBuf::from("powershell.exe"),
            args: vec![
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                OsString::from(r"C:\Tools\claude.ps1"),
                OsString::from("auth"),
                OsString::from("status"),
            ],
        }
    );
}

#[test]
fn bat_extension_uses_cmd_script_kind() {
    let shim = ClaudeExecutable::from_path(r"C:\Tools\claude.bat");
    assert_eq!(shim.kind(), ClaudeInvocationKind::CmdScript);
}

#[test]
fn cmd_shim_argv_encodes_special_characters() {
    let shim = ClaudeExecutable::from_path(r"C:\shims\claude.cmd");
    let cases: &[(&str, &str)] = &[
        ("a b", r#""a b""#),
        ("", r#""""#),
        ("a&b", r#""a&b""#),
        ("c|d", r#""c|d""#),
        ("e(f)", r#""e(f)""#),
        ("x^y", r#""x^y""#),
        ("wow!", r#""wow!""#),
        (r#"say "hi""#, r#""say ""hi""""#),
        ("100%sure", "100%%cd:~,%sure"),
        ("%PATH%", "%%cd:~,%PATH%%cd:~,%"),
        ("模型", "模型"),
        ("café", "café"),
        (r"C:\path\", r#""C:\path\\""#),
        // System-prompt-file style path with spaces must stay one quoted token.
        (
            r"C:\Users\me\run dir\system-prompt.txt",
            r#""C:\Users\me\run dir\system-prompt.txt""#,
        ),
        ("p>q", r#""p>q""#),
        ("r<s", r#""r<s""#),
        ("a;b", r#""a;b""#),
        ("a,b", r#""a,b""#),
        ("a=b", r#""a=b""#),
    ];
    for &(arg, needle) in cases {
        let plan = shim.plan([arg]).unwrap();
        let line =
            crate::claude_runtime::windows_shim_args::bat_command_line(&plan.program, &plan.args)
                .unwrap()
                .to_string_lossy()
                .into_owned();
        assert!(
            line.contains(needle),
            "arg={arg:?} needle={needle:?} line={line}"
        );
    }
}

#[test]
fn cmd_shim_encodes_system_prompt_file_path_with_spaces() {
    let shim = ClaudeExecutable::from_path(r"C:\Program Files\Claude\claude.cmd");
    let plan = shim
        .plan([
            "-p",
            "--system-prompt-file",
            r"C:\Users\me\AppData\Local\rho\runs\run 1\system-prompt.txt",
        ])
        .unwrap();
    let line =
        crate::claude_runtime::windows_shim_args::bat_command_line(&plan.program, &plan.args)
            .unwrap()
            .to_string_lossy()
            .into_owned();
    assert!(
        line.contains(r#""C:\Program Files\Claude\claude.cmd""#),
        "{line}"
    );
    assert!(
        line.contains(r#""C:\Users\me\AppData\Local\rho\runs\run 1\system-prompt.txt""#),
        "{line}"
    );
    assert!(line.contains("--system-prompt-file"), "{line}");
}

#[test]
fn cmd_shim_rejects_cr_lf_before_spawn() {
    let shim = ClaudeExecutable::from_path(r"C:\shims\claude.cmd");
    let err = shim.plan(["ok\nbad"]).unwrap_err();
    assert!(matches!(
        err,
        ClaudeExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
    ));
    let err = shim.try_command(["ok\rbad"]).unwrap_err();
    assert!(matches!(
        err,
        ClaudeExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
    ));
}

#[test]
fn powershell_try_command_accepts_metacharacters() {
    let shim = ClaudeExecutable::from_path(r"C:\Tools\claude.ps1");
    let plan = shim.plan(["a&b", "%PATH%", "x y", "模型", ""]).unwrap();
    assert_eq!(plan.program, PathBuf::from("powershell.exe"));
    assert!(plan.args.iter().any(|a| a == "a&b"));
    assert!(plan.args.iter().any(|a| a == "%PATH%"));
    assert!(plan.args.iter().any(|a| a == "x y"));
    assert!(plan.args.iter().any(|a| a == "模型"));
    assert!(plan.args.iter().any(|a| a.is_empty()));
}

#[test]
fn direct_command_builder_preserves_args() {
    let exe = ClaudeExecutable::from_path("/usr/bin/claude");
    // Construction must not fail; spawn is not required here.
    let _ = exe.try_command(["auth", "status"]).unwrap();
    let _ = exe.try_command(["--version"]).unwrap();
}

#[cfg(unix)]
#[test]
fn resolve_named_absolute_file_uses_direct_invocation() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("claude");
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let resolved = resolve_named(program.to_str().unwrap()).unwrap();
    assert_eq!(resolved.kind(), ClaudeInvocationKind::Direct);
    assert_eq!(resolved.program(), program.as_path());
}

/// On Windows CI, spawn fake `.cmd` and `.ps1` shims that write argv to a file
/// and assert exact round-trip of special characters.
#[cfg(windows)]
mod windows_round_trip {
    use std::process::Stdio;
    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use super::{
        ClaudeExecutable, ClaudeExecutableError, ClaudeInvocationKind, WindowsShimArgError,
    };

    /// Fake Claude `.cmd` shim: forward `%*` to a native argv consumer.
    ///
    /// Real npm/scoop `claude.cmd` shims end with `node.exe ... %*`. Observing
    /// batch `%1` is the wrong boundary: cmd keeps surrounding quotes in `%1`
    /// (`ARG2="a b"`), while CRT / PowerShell `-File` / Node `process.argv`
    /// dequote. Probe that native boundary so expectations match what Claude
    /// receives. Delayed expansion stays off; extensions stay on so the std
    /// `%` null-slice hack can restore literal percent chars before forward.
    fn write_cmd_recorder(path: &std::path::Path, out_file: &std::path::Path) {
        let probe = path.with_file_name("rho_cmd_argv_probe.ps1");
        let out = out_file.display().to_string().replace('/', "\\");
        std::fs::write(
            &probe,
            "$ErrorActionPreference = 'Stop'\r\n\
$outPath = $env:RHO_CMD_ARGV_OUT\r\n\
if ([string]::IsNullOrEmpty($outPath)) { throw 'RHO_CMD_ARGV_OUT missing' }\r\n\
$lines = New-Object System.Collections.Generic.List[string]\r\n\
foreach ($a in $args) { [void]$lines.Add([string]$a) }\r\n\
[System.IO.File]::WriteAllLines($outPath, $lines)\r\n",
        )
        .unwrap();
        let script = format!(
            "@echo off\r\n\
setlocal EnableExtensions DisableDelayedExpansion\r\n\
set \"RHO_CMD_ARGV_OUT={out}\"\r\n\
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0rho_cmd_argv_probe.ps1\" %*\r\n\
exit /b %ERRORLEVEL%\r\n"
        );
        std::fs::write(path, script).unwrap();
    }

    fn write_ps1_recorder(path: &std::path::Path, out_file: &std::path::Path) {
        let out = out_file.display().to_string().replace('/', "\\");
        // $args is the bound argument array for -File invocations.
        let script = format!(
            "$ErrorActionPreference = 'Stop'\r\n\
$out = @()\r\n\
foreach ($a in $args) {{ $out += $a }}\r\n\
[System.IO.File]::WriteAllLines('{out}', $out)\r\n"
        );
        std::fs::write(path, script).unwrap();
    }

    async fn run_and_read(command: &mut tokio::process::Command, out: &std::path::Path) -> String {
        let _ = std::fs::remove_file(out);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = tokio::time::timeout(Duration::from_secs(15), command.status())
            .await
            .expect("shim timed out")
            .expect("shim spawn failed");
        assert!(status.success(), "shim exit {status}");
        std::fs::read_to_string(out).expect("shim output missing")
    }

    #[tokio::test]
    async fn cmd_shim_round_trips_special_argv() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("claude.cmd");
        let out = directory.path().join("argv.txt");
        write_cmd_recorder(&shim, &out);

        let exe = ClaudeExecutable::from_path(&shim);
        assert_eq!(exe.kind(), ClaudeInvocationKind::CmdScript);

        let args = [
            "auth",
            "a b",
            "a&b",
            "c|d",
            "e(f)",
            "wow!",
            "100%sure",
            r#"say "hi""#,
            "模型",
        ];
        let mut command = exe.try_command(args).unwrap();
        let body = run_and_read(&mut command, &out).await;
        // Native-boundary argv (PowerShell -File after %* forward), not raw %1.
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines,
            vec![
                "auth",
                "a b",
                "a&b",
                "c|d",
                "e(f)",
                "wow!",
                "100%sure",
                "say \"hi\"",
                "模型",
            ],
            "{body}"
        );
    }

    #[tokio::test]
    async fn cmd_shim_round_trips_prompt_file_path_and_metacharacters() {
        let directory = tempfile::tempdir().unwrap();
        let run_dir = directory.path().join("run dir");
        std::fs::create_dir_all(&run_dir).unwrap();
        let prompt_file = run_dir.join("system-prompt.txt");
        std::fs::write(&prompt_file, "multi\nline").unwrap();

        let shim = directory.path().join("claude.cmd");
        let out = directory.path().join("argv.txt");
        write_cmd_recorder(&shim, &out);

        let exe = ClaudeExecutable::from_path(&shim);
        let prompt_path = prompt_file.to_string_lossy().into_owned();
        let args = [
            "-p",
            "--system-prompt-file",
            prompt_path.as_str(),
            "a&b",
            "c|d",
            "e(f)",
            "x^y",
            "p>q",
            "wow!",
        ];
        let mut command = exe.try_command(args).unwrap();
        let body = run_and_read(&mut command, &out).await;
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[0], "-p", "{body}");
        assert_eq!(lines[1], "--system-prompt-file", "{body}");
        // Path with spaces must round-trip as one dequoted argument.
        assert_eq!(lines[2], prompt_path, "{body}");
        assert_eq!(
            &lines[3..],
            ["a&b", "c|d", "e(f)", "x^y", "p>q", "wow!"],
            "{body}"
        );
    }

    #[tokio::test]
    async fn ps1_shim_round_trips_special_argv() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("claude.ps1");
        let out = directory.path().join("argv.txt");
        write_ps1_recorder(&shim, &out);

        let exe = ClaudeExecutable::from_path(&shim);
        assert_eq!(exe.kind(), ClaudeInvocationKind::PowerShellScript);

        let args = [
            "auth",
            "a b",
            "a&b",
            "c|d",
            "e(f)",
            "wow!",
            "100%sure",
            r#"say "hi""#,
            "模型",
            "",
        ];
        let mut command = exe.try_command(args).unwrap();
        let body = run_and_read(&mut command, &out).await;
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines,
            vec![
                "auth",
                "a b",
                "a&b",
                "c|d",
                "e(f)",
                "wow!",
                "100%sure",
                "say \"hi\"",
                "模型",
                "",
            ],
            "{body}"
        );
    }

    #[tokio::test]
    async fn ps1_shim_round_trips_prompt_file_path_with_spaces() {
        let directory = tempfile::tempdir().unwrap();
        let run_dir = directory.path().join("run dir");
        std::fs::create_dir_all(&run_dir).unwrap();
        let prompt_file = run_dir.join("system-prompt.txt");
        std::fs::write(&prompt_file, "multi\nline").unwrap();

        let shim = directory.path().join("claude.ps1");
        let out = directory.path().join("argv.txt");
        write_ps1_recorder(&shim, &out);

        let exe = ClaudeExecutable::from_path(&shim);
        let prompt_path = prompt_file.to_string_lossy().into_owned();
        let args = [
            "-p",
            "--system-prompt-file",
            prompt_path.as_str(),
            "a&b",
            "%PATH%",
            "x^y",
            "p>q",
            "r<s",
        ];
        let mut command = exe.try_command(args).unwrap();
        let body = run_and_read(&mut command, &out).await;
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[0], "-p", "{body}");
        assert_eq!(lines[1], "--system-prompt-file", "{body}");
        assert_eq!(lines[2], prompt_path, "{body}");
        assert_eq!(lines[3], "a&b", "{body}");
        assert_eq!(lines[4], "%PATH%", "{body}");
        assert_eq!(lines[5], "x^y", "{body}");
        assert_eq!(lines[6], "p>q", "{body}");
        assert_eq!(lines[7], "r<s", "{body}");
    }

    #[tokio::test]
    async fn cmd_shim_try_command_rejects_newline_arg() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("claude.cmd");
        let out = directory.path().join("argv.txt");
        write_cmd_recorder(&shim, &out);
        let exe = ClaudeExecutable::from_path(&shim);
        let err = exe.try_command(["ok\nbad"]).unwrap_err();
        assert!(matches!(
            err,
            ClaudeExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
        ));
    }
}
