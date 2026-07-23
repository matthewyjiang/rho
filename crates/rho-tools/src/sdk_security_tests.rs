use rho_sdk::{ExecutableSelection, ProcessInvocation};

#[cfg(unix)]
#[test]
fn process_authorization_uses_the_unix_execution_plan() {
    let command = "printf '%s' hello";

    let invocation = crate::shell_invocation(command);

    assert_eq!(
        invocation,
        ProcessInvocation::Shell {
            executable: "bash".into(),
            selection: ExecutableSelection::SearchPath,
            arguments: vec!["-lc".into()],
            command: command.into(),
        }
    );
}

#[cfg(windows)]
#[test]
fn process_authorization_uses_the_wrapped_windows_execution_plan() {
    let command = "Write-Output hello";

    let invocation = crate::shell_invocation(command);

    assert_eq!(
        invocation,
        ProcessInvocation::Shell {
            executable: "powershell.exe".into(),
            selection: ExecutableSelection::SearchPath,
            arguments: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
            ],
            command: crate::powershell::wrapped_command(command),
        }
    );
}
