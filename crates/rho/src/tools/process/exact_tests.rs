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

// Covers: workflow commands must keep stdout and stderr separate and retain a typed exit code.
// Owner: exact workflow process adapter.
#[cfg(unix)]
#[tokio::test]
async fn captures_separate_bounded_streams_and_exit_code() {
    let output = run_exact_process(
        shell_execution("printf 123456; printf abcdef >&2; exit 7", 4),
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
    let output = run_exact_process(shell_execution("cat", 16), &cancellation)
        .await
        .unwrap();

    assert_eq!(output.exit, ExactProcessExit::Cancellation);
}
