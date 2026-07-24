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
        unix.argv(["auth", "status"]),
        ClaudeArgv {
            program: PathBuf::from("/usr/bin/claude"),
            args: vec![OsString::from("auth"), OsString::from("status")],
            windows_command_line: None,
        }
    );

    let exe = ClaudeExecutable::from_path(r"C:\Tools\claude.exe");
    assert_eq!(exe.kind(), ClaudeInvocationKind::Direct);
    assert_eq!(
        exe.argv(["--version"]),
        ClaudeArgv {
            program: PathBuf::from(r"C:\Tools\claude.exe"),
            args: vec![OsString::from("--version")],
            windows_command_line: None,
        }
    );
}

#[test]
fn classifies_cmd_shim_uses_script_image_and_bat_command_line() {
    let shim = ClaudeExecutable::from_path(r"C:\Users\me\scoop\shims\claude.cmd");
    assert_eq!(shim.kind(), ClaudeInvocationKind::CmdScript);
    let plan = shim.argv(["auth", "logout"]);
    assert_eq!(
        plan.program,
        PathBuf::from(r"C:\Users\me\scoop\shims\claude.cmd")
    );
    assert_eq!(
        plan.args,
        vec![OsString::from("auth"), OsString::from("logout")]
    );
    let line = plan
        .windows_command_line
        .expect("cmd shim exposes bat command line")
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
        shim.argv(["auth", "status"]),
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
            windows_command_line: None,
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
        let line = shim
            .argv([arg])
            .windows_command_line
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
        .try_argv([
            "-p",
            "--system-prompt-file",
            r"C:\Users\me\AppData\Local\rho\runs\run 1\system-prompt.txt",
        ])
        .unwrap();
    let line = plan
        .windows_command_line
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
    let err = shim.try_argv(["ok\nbad"]).unwrap_err();
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
    let plan = shim.try_argv(["a&b", "%PATH%", "x y", "模型", ""]).unwrap();
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
    // Construction must not panic; spawn is not required here.
    let _ = exe.command(["auth", "status"]);
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

    fn write_cmd_recorder(path: &std::path::Path, out_file: &std::path::Path) {
        // Dump %1..%9 after cmd parse. Delayed expansion off; extensions on
        // so the std % null-slice hack can restore literal percent chars.
        let out = out_file.display().to_string().replace('/', "\\");
        let script = format!(
            "@echo off\r\n\
setlocal EnableExtensions DisableDelayedExpansion\r\n\
set \"OUT={out}\"\r\n\
>\"%OUT%\" echo ARG1=%1\r\n\
>>\"%OUT%\" echo ARG2=%2\r\n\
>>\"%OUT%\" echo ARG3=%3\r\n\
>>\"%OUT%\" echo ARG4=%4\r\n\
>>\"%OUT%\" echo ARG5=%5\r\n\
>>\"%OUT%\" echo ARG6=%6\r\n\
>>\"%OUT%\" echo ARG7=%7\r\n\
>>\"%OUT%\" echo ARG8=%8\r\n\
>>\"%OUT%\" echo ARG9=%9\r\n\
exit /b 0\r\n"
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
        // %1..%9 lines; cmd dequotes standard quoted args.
        assert!(body.contains("ARG1=auth"), "{body}");
        assert!(body.contains("ARG2=a b"), "{body}");
        assert!(body.contains("ARG3=a&b"), "{body}");
        assert!(body.contains("ARG4=c|d"), "{body}");
        assert!(body.contains("ARG5=e(f)"), "{body}");
        assert!(body.contains("ARG6=wow!"), "{body}");
        // % expansion null-slice should restore literal percent for the batch arg.
        assert!(body.contains("ARG7=100%sure"), "{body}");
        assert!(body.contains("ARG8=say \"hi\""), "{body}");
        assert!(body.contains("ARG9=模型"), "{body}");
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
        assert!(body.contains("ARG1=-p"), "{body}");
        assert!(body.contains("ARG2=--system-prompt-file"), "{body}");
        // Path with spaces must round-trip as one argument.
        assert!(
            body.contains(&format!("ARG3={prompt_path}"))
                || body.lines().any(|line| {
                    line.starts_with("ARG3=")
                        && line.contains("system-prompt.txt")
                        && line.contains("run dir")
                }),
            "{body}"
        );
        assert!(body.contains("ARG4=a&b"), "{body}");
        assert!(body.contains("ARG5=c|d"), "{body}");
        assert!(body.contains("ARG6=e(f)"), "{body}");
        assert!(body.contains("ARG7=x^y"), "{body}");
        assert!(body.contains("ARG8=p>q"), "{body}");
        assert!(body.contains("ARG9=wow!"), "{body}");
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
