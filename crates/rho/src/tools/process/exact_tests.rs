use super::*;

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
fn shell_execution(script: &str, max_output_bytes: usize) -> ProcessExecution {
    ProcessExecution::new(
        std::env::current_dir().unwrap(),
        rho_sdk::ProcessInvocation::executable("/bin/sh", vec!["-c".to_owned(), script.to_owned()]),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(max_output_bytes, None),
    )
}

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
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
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
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
            cleanup_incomplete: false,
        }
    );
}

// Covers: cancellation before process progress must yield the typed cancellation exit.
// Owner: exact workflow process adapter.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
#[tokio::test]
async fn maps_cancellation_to_typed_exit() {
    let cancellation = rho_sdk::CancellationToken::new();
    cancellation.cancel();
    let execution = shell_execution("while :; do :; done", 16);
    let (executable, cwd) = identities(&execution);
    let output = run_exact_process(execution, &executable, &cwd, &cancellation)
        .await
        .unwrap();

    assert_eq!(output.exit, ExactProcessExit::Cancellation);
}

// Covers: an executable replaced after authorization must not reach spawn.
// Owner: exact workflow process adapter identity gate.
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
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
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
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
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
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

// Covers: substitutions after identity verification must not change the
// executable, interpreter, or cwd used by the child.
// Owner: Linux exact workflow process launch adapter.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn launches_all_identity_paths_from_verified_handles() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let interpreter = directory.path().join("interpreter");
    std::fs::copy("/bin/sh", &interpreter).unwrap();
    std::fs::set_permissions(&interpreter, std::fs::Permissions::from_mode(0o700)).unwrap();
    let executable = directory.path().join("command");
    std::fs::write(
        &executable,
        format!("#!{}\nprintf original:; cat marker", interpreter.display()),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = directory.path().join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("marker"), "verified").unwrap();
    let execution = ProcessExecution::new(
        cwd.clone(),
        rho_sdk::ProcessInvocation::executable(&executable, Vec::new()),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(64, None),
    );
    let identity = crate::workflow::freeze_executable_identity(&executable).unwrap();
    let cwd_identity = crate::workflow::freeze_directory_identity(&cwd).unwrap();
    let verified_executable = crate::workflow::verify_executable_identity(&identity).unwrap();
    let verified_cwd = crate::workflow::verify_directory_identity(&cwd_identity).unwrap();

    std::fs::rename(&executable, directory.path().join("old-command")).unwrap();
    std::fs::write(&executable, "#!/bin/sh\nprintf replacement").unwrap();
    std::fs::rename(&interpreter, directory.path().join("old-interpreter")).unwrap();
    std::fs::copy("/bin/false", &interpreter).unwrap();
    std::fs::rename(&cwd, directory.path().join("old-cwd")).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("marker"), "replacement").unwrap();

    let mut command = command_from_execution(&execution, &verified_executable).unwrap();
    command
        .current_dir(crate::workflow::verified_handle_path(&verified_cwd.file, &cwd).unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    rho_tools::apply_process_environment(&mut command, execution.environment()).unwrap();
    crate::workflow::configure_handle_inheritance(
        &mut command,
        &[
            &verified_executable.executable.file,
            &verified_executable.interpreter.as_ref().unwrap().file,
            &verified_cwd.file,
        ],
    )
    .unwrap();
    let child = command.spawn().unwrap();
    let output = child.wait_with_output().await.unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"original:verified");
}

// Covers: a stable symlink in a script shebang must resolve to the same frozen
// interpreter identity at launch.
// Owner: exact workflow process identity verification.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn accepts_a_stable_symlinked_shebang_interpreter() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("script");
    std::fs::write(&script, "#!/bin/sh\nprintf stable").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let executable_identity = crate::workflow::freeze_executable_identity(&script).unwrap();
    let cwd_identity = crate::workflow::freeze_directory_identity(directory.path()).unwrap();
    let execution = ProcessExecution::new(
        directory.path(),
        rho_sdk::ProcessInvocation::executable(&script, Vec::new()),
        rho_sdk::ProcessEnvironment::Empty,
        rho_sdk::ProcessOutputLimits::new(1024, None),
    );

    let output = run_exact_process(
        execution,
        &executable_identity,
        &cwd_identity,
        &rho_sdk::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(output.exit, ExactProcessExit::Code(0));
    assert_eq!(output.stdout, b"stable");
}

// Covers: a supported target without a handle-based launch adapter must not
// run a frozen workflow command from a rechecked path.
// Owner: exact workflow process platform gate.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
#[test]
fn unsupported_platform_fails_closed() {
    let error = ensure_handle_based_launch_supported().unwrap_err();
    assert_eq!(error.kind(), ToolErrorKind::Execution);
}
