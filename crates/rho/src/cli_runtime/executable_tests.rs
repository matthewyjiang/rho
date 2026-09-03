use std::ffi::OsString;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::cli_runtime::windows_shim_args::WindowsShimArgError;

// Covers: non-existent program name returns None from resolve_named.
// Owner: pure unit
#[test]
fn missing_program_resolves_to_none() {
    for name in [
        "definitely-not-an-external-cli-binary-xyz",
        "/tmp/rho-missing-cli-binary-xyz",
    ] {
        assert!(resolve_named(name).is_none(), "name={name}");
    }
}

// Covers: .cmd extension classifies as CmdScript and plans bat command line.
// Owner: pure unit
#[test]
fn classifies_cmd_shim_and_plans_bat_command_line() {
    let shim = CliExecutable::from_path(r"C:\Users\me\scoop\shims\agent.cmd");
    assert_eq!(shim.kind(), CliInvocationKind::CmdScript);
    let plan = shim.plan(["status", "--verbose"]).unwrap();
    assert_eq!(
        plan.program,
        PathBuf::from(r"C:\Users\me\scoop\shims\agent.cmd")
    );
    assert_eq!(
        plan.args,
        vec![OsString::from("status"), OsString::from("--verbose")]
    );
}

// Covers: .ps1 extension classifies as PowerShellScript and constructs structured argv.
// Owner: pure unit
#[test]
fn classifies_ps1_shim_as_powershell_invocation() {
    let shim = CliExecutable::from_path(r"C:\Tools\agent.ps1");
    assert_eq!(shim.kind(), CliInvocationKind::PowerShellScript);
    let plan = shim.plan(["run", "task"]).unwrap();
    assert_eq!(
        plan,
        CliArgv {
            program: PathBuf::from("powershell.exe"),
            args: vec![
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                OsString::from(r"C:\Tools\agent.ps1"),
                OsString::from("run"),
                OsString::from("task"),
            ],
        }
    );
}

// Covers: direct executable without shim extension plans verbatim image and args.
// Owner: pure unit
#[test]
fn direct_executable_plans_verbatim_argv() {
    let exe = CliExecutable::from_path("/usr/local/bin/agent");
    assert_eq!(exe.kind(), CliInvocationKind::Direct);
    let plan = exe.plan(["--format", "json"]).unwrap();
    assert_eq!(plan.program, PathBuf::from("/usr/local/bin/agent"));
    assert_eq!(
        plan.args,
        vec![OsString::from("--format"), OsString::from("json")]
    );
}

// Covers: cmd shim plan rejects CR, LF, and NUL before CreateProcess.
// Owner: pure unit
#[test]
fn cmd_shim_rejects_disallowed_bytes() {
    let shim = CliExecutable::from_path(r"C:\shims\agent.cmd");
    assert_eq!(
        shim.plan(["bad\narg"]).unwrap_err(),
        CliExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
    );
    assert_eq!(
        shim.plan(["bad\rarg"]).unwrap_err(),
        CliExecutableError::WindowsShim(WindowsShimArgError::CmdDisallowedByte)
    );
}

// Covers: powershell shim plan rejects NUL.
// Owner: pure unit
#[test]
fn ps1_shim_rejects_nul() {
    let shim = CliExecutable::from_path(r"C:\Tools\agent.ps1");
    assert_eq!(
        shim.plan(["bad\0arg"]).unwrap_err(),
        CliExecutableError::WindowsShim(WindowsShimArgError::PowerShellDisallowedByte)
    );
}

// Covers: try_command constructs a Command from valid plan.
// Owner: pure unit
#[test]
fn try_command_constructs_command() {
    let exe = CliExecutable::from_path("/usr/bin/echo");
    let cmd = exe.try_command(["hello"]).unwrap();
    let _ = cmd;
}
