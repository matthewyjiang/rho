use std::ffi::OsString;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::cli_runtime::windows_shim_args::bat_command_line;

/// Covers: a bare name that is not on PATH and an explicit path that does not
/// exist both resolve to nothing; resolution never invents a binary.
/// Owner: `CliExecutable::resolve`.
#[test]
fn missing_program_resolves_to_none() {
    for name in [
        "definitely-not-an-external-cli-binary-xyz",
        "/tmp/rho-missing-cli-binary-xyz",
    ] {
        assert!(CliExecutable::resolve(name).is_none(), "name={name}");
    }
}

/// Covers: a `.cmd` shim keeps the script as the spawn image with verbatim argv,
/// and the encoded line is std's `cmd.exe /e:ON /v:OFF /d /c` wrapper rather
/// than a bare `cmd /C` that cmd re-parses loosely.
/// Owner: `CliExecutable::plan` for `CmdScript`.
#[test]
fn cmd_shim_uses_script_image_and_bat_command_line() {
    let shim = CliExecutable::from_path(r"C:\Users\me\scoop\shims\agent.cmd");
    assert_eq!(shim.kind(), CliInvocationKind::CmdScript);
    let plan = shim.plan(["auth", "logout"]).unwrap();
    assert_eq!(
        plan,
        CliArgv {
            program: PathBuf::from(r"C:\Users\me\scoop\shims\agent.cmd"),
            args: vec![OsString::from("auth"), OsString::from("logout")],
        }
    );
    let line = bat_command_line(&plan.program, &plan.args)
        .expect("cmd shim encodes bat command line")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        line,
        r#"cmd.exe /e:ON /v:OFF /d /c ""C:\Users\me\scoop\shims\agent.cmd" auth logout""#
    );
}

/// Covers: a `.ps1` shim is launched through `powershell.exe -File` with the
/// script and args as structured argv.
/// Owner: `CliExecutable::plan` for `PowerShellScript`.
#[test]
fn ps1_shim_plans_fixed_argv_powershell_invocation() {
    let shim = CliExecutable::from_path(r"C:\Tools\agent.ps1");
    assert_eq!(shim.kind(), CliInvocationKind::PowerShellScript);
    assert_eq!(
        shim.plan(["auth", "status"]).unwrap(),
        CliArgv {
            program: PathBuf::from("powershell.exe"),
            args: vec![
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                OsString::from(r"C:\Tools\agent.ps1"),
                OsString::from("auth"),
                OsString::from("status"),
            ],
        }
    );
}

/// Covers: a program without a shim extension spawns directly with verbatim argv.
/// Owner: `CliExecutable::plan` for `Direct`.
#[test]
fn direct_executable_plans_verbatim_argv() {
    let exe = CliExecutable::from_path("/usr/local/bin/agent");
    assert_eq!(exe.kind(), CliInvocationKind::Direct);
    assert_eq!(
        exe.plan(["--format", "json"]).unwrap(),
        CliArgv {
            program: PathBuf::from("/usr/local/bin/agent"),
            args: vec![OsString::from("--format"), OsString::from("json")],
        }
    );
}

/// Covers: every cmd metacharacter, quote, percent, and path-with-spaces case
/// is encoded so it survives cmd's re-parse as one literal token.
/// Owner: the bat encoder as driven through `plan`; the exact algorithm lives
/// in `windows_shim_args_tests`.
#[test]
fn cmd_shim_argv_encodes_special_characters() {
    let shim = CliExecutable::from_path(r"C:\shims\agent.cmd");
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
        let line = bat_command_line(&plan.program, &plan.args)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            line.contains(needle),
            "arg={arg:?} needle={needle:?} line={line}"
        );
    }
}

/// Covers: CR and LF in a cmd shim argument are rejected by `plan` and
/// `try_command` before any process exists, since they truncate the bat line.
/// Owner: `CliExecutable::plan` for `CmdScript`.
#[test]
fn cmd_shim_rejects_cr_lf_before_spawn() {
    let shim = CliExecutable::from_path(r"C:\shims\agent.cmd");
    assert_eq!(
        shim.plan(["ok\nbad"]).unwrap_err(),
        CliExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
    );
    assert!(matches!(
        shim.try_command(["ok\rbad"]).unwrap_err(),
        CliExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
    ));
}

/// Covers: PowerShell `-File` argv stays structured, so metacharacters, percent,
/// spaces, unicode, and empty strings pass through untouched; only NUL is fatal.
/// Owner: `CliExecutable::plan` for `PowerShellScript`.
#[test]
fn ps1_shim_keeps_metacharacters_and_rejects_nul() {
    let shim = CliExecutable::from_path(r"C:\Tools\agent.ps1");
    let plan = shim.plan(["a&b", "%PATH%", "x y", "模型", ""]).unwrap();
    assert_eq!(plan.program, PathBuf::from("powershell.exe"));
    assert_eq!(
        &plan.args[6..],
        [
            OsString::from("a&b"),
            OsString::from("%PATH%"),
            OsString::from("x y"),
            OsString::from("模型"),
            OsString::from(""),
        ]
    );
    assert_eq!(
        shim.plan(["bad\0arg"]).unwrap_err(),
        CliExecutableError::WindowsShim(WindowsShimArgError::PowerShellDisallowedByte)
    );
}

/// Covers: an explicit absolute path to a real file resolves as a direct
/// invocation of that exact file.
/// Owner: `CliExecutable::resolve` explicit-path branch.
#[cfg(unix)]
#[test]
fn resolve_absolute_file_uses_direct_invocation() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("agent");
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let resolved = CliExecutable::resolve(program.to_str().unwrap()).unwrap();
    assert_eq!(resolved.kind(), CliInvocationKind::Direct);
    assert_eq!(resolved.path(), program.as_path());
}

/// On Windows CI, spawn fake `.cmd` and `.ps1` shims that write argv to a file
/// and assert exact round-trip of special characters.
#[cfg(windows)]
mod windows_round_trip {
    use std::process::Stdio;
    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use super::{CliExecutable, CliExecutableError, CliInvocationKind, WindowsShimArgError};

    /// Fake `.cmd` shim: forward `%*` to a native argv consumer.
    ///
    /// Real npm/scoop `.cmd` shims end with `node.exe ... %*`. Observing
    /// batch `%1` is the wrong boundary: cmd keeps surrounding quotes in `%1`
    /// (`ARG2="a b"`), while CRT / PowerShell `-File` / Node `process.argv`
    /// dequote. Probe that native boundary so expectations match what the tool
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

    // Healthy Windows CI runs complete these PowerShell probes in 4 to 6 seconds.
    // A loaded runner exceeded 15 seconds, so use 10 times the measured healthy
    // upper bound as a hang tripwire rather than a startup-speed assertion.
    const WINDOWS_SHIM_PROCESS_EXIT_BUDGET: Duration = Duration::from_secs(60);

    async fn run_and_read(command: &mut tokio::process::Command, out: &std::path::Path) -> String {
        let _ = std::fs::remove_file(out);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = tokio::time::timeout(WINDOWS_SHIM_PROCESS_EXIT_BUDGET, command.status())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "Windows shim process did not exit within the {WINDOWS_SHIM_PROCESS_EXIT_BUDGET:?} test budget"
                )
            })
            .expect("shim spawn failed");
        assert!(status.success(), "shim exit {status}");
        std::fs::read_to_string(out).expect("shim output missing")
    }

    #[tokio::test]
    async fn cmd_shim_round_trips_special_argv() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("agent.cmd");
        let out = directory.path().join("argv.txt");
        write_cmd_recorder(&shim, &out);

        let exe = CliExecutable::from_path(&shim);
        assert_eq!(exe.kind(), CliInvocationKind::CmdScript);

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

        let shim = directory.path().join("agent.cmd");
        let out = directory.path().join("argv.txt");
        write_cmd_recorder(&shim, &out);

        let exe = CliExecutable::from_path(&shim);
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
        let shim = directory.path().join("agent.ps1");
        let out = directory.path().join("argv.txt");
        write_ps1_recorder(&shim, &out);

        let exe = CliExecutable::from_path(&shim);
        assert_eq!(exe.kind(), CliInvocationKind::PowerShellScript);

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

        let shim = directory.path().join("agent.ps1");
        let out = directory.path().join("argv.txt");
        write_ps1_recorder(&shim, &out);

        let exe = CliExecutable::from_path(&shim);
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
        assert_eq!(
            &lines[3..],
            ["a&b", "%PATH%", "x^y", "p>q", "r<s"],
            "{body}"
        );
    }

    #[tokio::test]
    async fn cmd_shim_try_command_rejects_newline_arg() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory.path().join("agent.cmd");
        let out = directory.path().join("argv.txt");
        write_cmd_recorder(&shim, &out);
        let exe = CliExecutable::from_path(&shim);
        let err = exe.try_command(["ok\nbad"]).unwrap_err();
        assert!(matches!(
            err,
            CliExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
        ));
    }
}
