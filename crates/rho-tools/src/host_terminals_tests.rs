use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_sdk::tool::tool_progress_channel;

use super::*;
use crate::host::HostError;

/// How the fake terminal's command ends.
#[derive(Clone, Copy)]
enum Ending {
    Exits(i32),
    /// Exits with the code once the time passes.
    ExitsAfter(i32, Duration),
    Never,
    /// The host fails the wait while the command may still run.
    WaitFails,
}

/// Records each host request, by name, in the order the runner sends them
/// (polls them, as a connection port does).
#[derive(Clone)]
struct FakeTerminals {
    ending: Ending,
    /// What `output` returns: the text, and whether the host dropped its
    /// beginning.
    output: (&'static str, bool),
    log: Arc<Mutex<Vec<&'static str>>>,
}

struct FakeTerminal(FakeTerminals);

impl HostTerminals for FakeTerminals {
    fn create(&self, _command: HostCommand) -> HostFuture<'_, Box<dyn HostTerminal>> {
        Box::pin(async {
            self.log.lock().unwrap().push("create");
            Ok(Box::new(FakeTerminal(self.clone())) as Box<dyn HostTerminal>)
        })
    }
}

impl HostTerminal for FakeTerminal {
    fn id(&self) -> &str {
        "term-1"
    }

    fn wait_for_exit(&self) -> HostFuture<'_, HostExitStatus> {
        let ending = self.0.ending;
        Box::pin(async move {
            self.0.log.lock().unwrap().push("wait_for_exit");
            match ending {
                Ending::Exits(code) => Ok(HostExitStatus {
                    exit_code: Some(code),
                    signal: None,
                }),
                Ending::ExitsAfter(code, after) => {
                    tokio::time::sleep(after).await;
                    Ok(HostExitStatus {
                        exit_code: Some(code),
                        signal: None,
                    })
                }
                Ending::Never => std::future::pending().await,
                Ending::WaitFails => Err(HostError::new("connection lost")),
            }
        })
    }

    fn output(&self) -> HostFuture<'_, HostTerminalOutput> {
        Box::pin(async {
            self.0.log.lock().unwrap().push("output");
            let (output, truncated) = self.0.output;
            Ok(HostTerminalOutput::new(output.into(), truncated))
        })
    }

    fn kill(&self) -> HostFuture<'_, ()> {
        Box::pin(async {
            self.0.log.lock().unwrap().push("kill");
            Ok::<(), HostError>(())
        })
    }
}

fn command() -> HostCommand {
    HostCommand {
        program: "bash".into(),
        args: vec!["-c".into(), "make".into()],
        env: Vec::new(),
        cwd: "/work".into(),
        output_byte_limit: 64_000,
    }
}

/// What a run returned, without its timing.
#[derive(Debug, PartialEq)]
enum Returned {
    Finished {
        failed: bool,
        stdout: String,
        exit_code: Option<i64>,
    },
    Error(String),
}

// Covers: a hosted command announces its terminal before waiting, reports a
// nonzero exit as a failed result with the host's truncation made visible,
// even when the timeout is due as it exits, and a timeout, cancellation, or
// failed wait kills the command instead of leaving it running in the host.
// Owner: host terminal runner
#[tokio::test(start_paused = true)]
async fn host_terminal_runs_report_exits_and_kill_on_timeout_or_cancel() {
    let cancelled = RunCancellation::new();
    cancelled.cancel();
    for (ending, timeout, cancellation, expected_log, expected) in [
        (
            Ending::Exits(2),
            None,
            RunCancellation::new(),
            vec!["create", "wait_for_exit", "output"],
            Returned::Finished {
                failed: true,
                stdout: "[earlier output truncated]\ntail".into(),
                exit_code: Some(2),
            },
        ),
        (
            // The paused clock reaches the exit and the timeout together.
            Ending::ExitsAfter(2, Duration::from_secs(5)),
            Some(Duration::from_secs(5)),
            RunCancellation::new(),
            vec!["create", "wait_for_exit", "output"],
            Returned::Finished {
                failed: true,
                stdout: "[earlier output truncated]\ntail".into(),
                exit_code: Some(2),
            },
        ),
        (
            Ending::Never,
            Some(Duration::from_millis(1)),
            RunCancellation::new(),
            vec!["create", "wait_for_exit", "kill", "output"],
            Returned::Error(
                "command timed out after 0s\n\nstdout:\n[earlier output truncated]\ntail\n".into(),
            ),
        ),
        (
            Ending::Never,
            None,
            cancelled,
            vec!["create", "kill"],
            Returned::Error(ToolError::Cancelled.to_string()),
        ),
        (
            Ending::WaitFails,
            None,
            RunCancellation::new(),
            vec!["create", "wait_for_exit", "kill"],
            Returned::Error("could not wait for the host terminal: connection lost".into()),
        ),
    ] {
        let terminals = FakeTerminals {
            ending,
            output: ("tail\n", true),
            log: Arc::default(),
        };
        let (progress, mut announced) =
            tool_progress_channel(std::num::NonZeroUsize::new(4).unwrap());
        let returned = match run(&terminals, command(), timeout, &cancellation, &progress).await {
            Ok((run, terminal)) => {
                assert_eq!(terminal, "term-1");
                let shown = crate::parse_shell_content(run.text());
                Returned::Finished {
                    failed: run.is_failure(),
                    stdout: shown.stdout,
                    exit_code: shown.exit_code,
                }
            }
            Err(error) => Returned::Error(error.to_string()),
        };
        assert_eq!(
            (terminals.log.lock().unwrap().clone(), returned),
            (expected_log, expected)
        );
        let announcement = announced.recv().await.unwrap();
        assert_eq!(
            announcement.presentation().host_terminal_id(),
            Some("term-1")
        );
    }
}

// Covers: output the host kept at its byte limit still fits the result, as
// text with its footer and as structured data, even when JSON escaping
// expands it: like the host, the result drops the oldest output, not the
// newest or the exit status.
// Owner: host terminal runner
#[tokio::test]
async fn host_output_at_the_limit_keeps_its_newest_lines() {
    let plain = (1..=40)
        .map(|line| format!("line {line}\n"))
        .collect::<String>();
    let colored = (1..=40)
        .map(|line| format!("\x1b[1m\x1b[31mline {line}\x1b[0m\n"))
        .collect::<String>();
    for (output, last_line) in [(plain, "line 40"), (colored, "line 40\x1b[0m")] {
        let terminals = FakeTerminals {
            ending: Ending::Exits(3),
            output: (Box::leak(output.into_boxed_str()), false),
            log: Arc::default(),
        };
        let mut limited = command();
        limited.output_byte_limit = 200;
        let (progress, _announced) = tool_progress_channel(std::num::NonZeroUsize::new(4).unwrap());

        let (run, _) = run(
            &terminals,
            limited,
            None,
            &RunCancellation::new(),
            &progress,
        )
        .await
        .unwrap();
        let run = run.limit_data(200).unwrap();
        let shown = crate::parse_shell_content(run.text());

        assert_eq!(
            (
                run.data().is_some(),
                run.text().len() <= 200,
                shown.stdout.starts_with("[earlier output truncated]\n"),
                shown.stdout.ends_with(last_line),
                shown.stdout.len() > "[earlier output truncated]\n".len() + last_line.len(),
                shown.exit_code,
            ),
            (true, true, true, true, true, Some(3)),
            "{last_line:?}"
        );
    }
}

// Covers: a host command keeps Rho's process policy: no stdin, no waiting
// pagers, withheld variables removed before bash starts, and a plan whose
// environment the host cannot reproduce is refused rather than run another
// way.
// Owner: host terminal command mapping
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn host_commands_keep_the_process_policy_or_are_refused() {
    let plan = |environment| {
        ProcessExecution::new(
            "/work",
            crate::shell_invocation("echo hi"),
            environment,
            rho_sdk::ProcessOutputLimits::new(1_000, None),
        )
    };
    let command = |args: &[&str]| {
        Ok(HostCommand {
            program: "env".into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: vec![
                ("PAGER".into(), "cat".into()),
                ("GIT_PAGER".into(), "cat".into()),
            ],
            cwd: "/work".into(),
            output_byte_limit: 1_000,
        })
    };
    let refused = |detail: &str| Err(format!("the host terminal cannot run {detail}"));
    let commands = [
        ProcessEnvironment::InheritAll,
        ProcessEnvironment::inherit_except(["OPENAI_API_KEY", "ANTHROPIC_API_KEY"]),
        ProcessEnvironment::inherit_except(["A=B"]),
        ProcessEnvironment::Empty,
    ]
    .map(|environment| host_command(&plan(environment)).map_err(|error| error.to_string()));
    assert_eq!(
        commands,
        [
            command(&["bash", "-c", "exec </dev/null\necho hi"]),
            command(&[
                "-u",
                "ANTHROPIC_API_KEY",
                "-u",
                "OPENAI_API_KEY",
                "bash",
                "-c",
                "exec </dev/null\necho hi",
            ]),
            refused("a command that must not see the variable `A=B`, which `env -u` cannot name"),
            refused("a command with a restricted environment"),
        ]
    );
}
