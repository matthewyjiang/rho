use super::*;

#[cfg(unix)]
fn shell_execution(script: &str, max_output_bytes: usize) -> ProcessExecution {
    ProcessExecution::new(
        std::env::current_dir().unwrap(),
        rho_sdk::ProcessInvocation::executable("/bin/sh", vec!["-c".to_owned(), script.to_owned()]),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(max_output_bytes, None),
    )
}

#[cfg(unix)]
fn identities(
    execution: &ProcessExecution,
) -> (
    crate::workflow::ExecutableIdentity,
    crate::workflow::FrozenPathIdentity,
) {
    (
        crate::workflow::freeze_executable_identity(execution.invocation().executable_path())
            .unwrap(),
        crate::workflow::freeze_directory_identity(execution.working_directory()).unwrap(),
    )
}

// Covers: workflow commands must keep stdout and stderr separate and retain a typed exit code.
// Owner: exact workflow process adapter.
#[cfg(unix)]
#[tokio::test]
async fn captures_separate_bounded_streams_and_exit_code() {
    let execution = shell_execution("printf 123456; printf abcdef >&2; exit 7", 4);
    let (executable, cwd) = identities(&execution);
    let output = run_exact_process(
        execution,
        &executable,
        &cwd,
        &rho_sdk::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        output,
        ExactProcessOutput {
            exit: ExactProcessExit::Code(7),
            stdout: b"1234".to_vec(),
            stderr: b"abcd".to_vec(),
            stdout_truncated: true,
            stderr_truncated: true,
            stdout_observed_bytes: 6,
            stderr_observed_bytes: 6,
        }
    );
}

// Covers: cancellation before process progress must yield the typed cancellation exit.
// Owner: exact workflow process adapter.
#[cfg(unix)]
#[tokio::test]
async fn maps_cancellation_to_typed_exit() {
    let cancellation = rho_sdk::CancellationToken::new();
    cancellation.cancel();
    let execution = shell_execution("cat", 16);
    let (executable, cwd) = identities(&execution);
    let output = run_exact_process(execution, &executable, &cwd, &cancellation)
        .await
        .unwrap();

    assert_eq!(output.exit, ExactProcessExit::Cancellation);
}

// Covers: an executable replaced after authorization must not reach spawn.
// Owner: exact workflow process adapter identity gate.
#[cfg(unix)]
#[tokio::test]
async fn rejects_executable_substitution_before_spawn() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("command");
    std::fs::write(&executable, "#!/bin/sh\nprintf original").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let execution = ProcessExecution::new(
        directory.path().to_owned(),
        rho_sdk::ProcessInvocation::executable(&executable, Vec::new()),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(16, None),
    );
    let (identity, cwd) = identities(&execution);
    let replacement = directory.path().join("replacement");
    std::fs::write(&replacement, "#!/bin/sh\nprintf replaced").unwrap();
    std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(&replacement, &executable).unwrap();

    let error = run_exact_process(
        execution,
        &identity,
        &cwd,
        &rho_sdk::CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), ToolErrorKind::Execution);
}

// Covers: a working directory replaced after authorization must not reach spawn.
// Owner: exact workflow process adapter identity gate.
#[cfg(unix)]
#[tokio::test]
async fn rejects_working_directory_substitution_before_spawn() {
    let parent = tempfile::tempdir().unwrap();
    let cwd = parent.path().join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    let execution = ProcessExecution::new(
        cwd.clone(),
        rho_sdk::ProcessInvocation::executable("/bin/sh", vec!["-c".into(), "pwd".into()]),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(64, None),
    );
    let (identity, cwd_identity) = identities(&execution);
    std::fs::rename(&cwd, parent.path().join("old-cwd")).unwrap();
    std::fs::create_dir(&cwd).unwrap();

    let error = run_exact_process(
        execution,
        &identity,
        &cwd_identity,
        &rho_sdk::CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), ToolErrorKind::Execution);
}

// Covers: a script interpreter replaced after authorization must not reach spawn.
// Owner: exact workflow process adapter identity gate.
#[cfg(unix)]
#[tokio::test]
async fn rejects_script_interpreter_substitution_before_spawn() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let interpreter = directory.path().join("interpreter");
    std::fs::copy("/bin/sh", &interpreter).unwrap();
    std::fs::set_permissions(&interpreter, std::fs::Permissions::from_mode(0o700)).unwrap();
    let script = directory.path().join("script");
    std::fs::write(
        &script,
        format!("#!{}\nprintf original", interpreter.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let execution = ProcessExecution::new(
        directory.path().to_owned(),
        rho_sdk::ProcessInvocation::executable(&script, Vec::new()),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(16, None),
    );
    let (identity, cwd) = identities(&execution);
    let replacement = directory.path().join("replacement-interpreter");
    std::fs::copy("/bin/false", &replacement).unwrap();
    std::fs::rename(replacement, interpreter).unwrap();

    let error = run_exact_process(
        execution,
        &identity,
        &cwd,
        &rho_sdk::CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), ToolErrorKind::Execution);
}
