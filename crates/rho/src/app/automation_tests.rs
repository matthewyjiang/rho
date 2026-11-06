use std::{io, num::NonZeroUsize};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    Rho, SessionOptions,
};
use serde_json::json;

use super::{
    classify_error, classify_run_terminal, prompt_from_reader, AutomationExit, RunTerminal,
    TerminalMessages,
};
use crate::app::{
    automation_protocol::TerminalReason,
    headless_host::{PromptEnd, StopCause},
    session_runtime::TurnOutcome,
    shutdown_signal::ShutdownSignal,
};

// Covers: terminal reasons come from typed errors (an AutomationExit or a
// fatal tool host error), never from parsing the message text.
// Owner: pure unit (run terminal classification)
#[test]
fn classifies_typed_errors_without_parsing_their_messages() {
    let exit = anyhow::Error::new(AutomationExit::new(
        1,
        TerminalReason::OutputError,
        "wording can change",
    ));
    let tool_host = anyhow::Error::new(rho_sdk::Error::Tool(rho_sdk::tool::ToolError::new(
        rho_sdk::tool::ToolErrorKind::Execution,
        "host failed",
    )));

    assert_eq!(classify_error(&exit), (TerminalReason::OutputError, 1));
    assert_eq!(
        classify_error(&tool_host),
        (TerminalReason::ToolHostError, 1)
    );
}

#[test]
fn terminal_messages_keep_detail_but_hide_authentication_errors() {
    let messages = TerminalMessages::for_selection("xai", "xai-oauth");
    let configuration = anyhow::anyhow!("TOML parse error at line 1, column 5");
    assert_eq!(
        messages.for_error(TerminalReason::ConfigurationError, &configuration),
        "TOML parse error at line 1, column 5"
    );

    let output = anyhow::anyhow!("could not write JSONL output: broken pipe");
    assert_eq!(
        messages.for_error(TerminalReason::OutputError, &output),
        "could not write JSONL output: broken pipe"
    );

    let provider = anyhow::anyhow!("provider unavailable");
    assert_eq!(
        messages.for_error(TerminalReason::ProviderError, &provider),
        "provider unavailable"
    );

    // The provider's text may echo credentials; only the selection is named,
    // offering only other keyed modes of that provider as `--auth` fixes.
    let authentication = anyhow::anyhow!("token fixture-secret rejected");
    for (provider, auth, expected) in [
        (
            "xai",
            "xai-oauth",
            "authentication failed for xai (auth: xai-oauth); run /login xai or pass --auth xai-api-key",
        ),
        (
            "openai-codex",
            "codex",
            "authentication failed for openai-codex (auth: codex); run /login openai-codex",
        ),
        (
            "xai",
            "xai-api-key",
            "authentication failed for xai (auth: xai-api-key); run /login xai or pass --auth xai-oauth",
        ),
    ] {
        assert_eq!(
            TerminalMessages::for_selection(provider, auth)
                .for_error(TerminalReason::Authentication, &authentication),
            expected,
            "{provider}/{auth}"
        );
    }
}

/// How the prompt itself ended, before any stop.
#[derive(Clone, Copy, Debug)]
enum Ended {
    Answered,
    AtStepLimit,
    Cancelled,
    Failed,
}

/// A prompt that ended as `ended`. Finished runs come from a scripted
/// session, so their stop reason is the SDK's own.
async fn prompt_that(ended: Ended) -> anyhow::Result<TurnOutcome> {
    let (turn, max_steps) = match ended {
        Ended::Cancelled => return Ok(TurnOutcome::Cancelled),
        Ended::Failed => return Err(anyhow::anyhow!("provider unavailable")),
        Ended::Answered => (
            ModelResponse::Assistant(vec![ContentBlock::Text("answer".into())]),
            None,
        ),
        Ended::AtStepLimit => (
            ModelResponse::Assistant(vec![ContentBlock::ToolCall(ToolCall::new(
                "call-1".into(),
                "noop".into(),
                json!({}),
            ))]),
            NonZeroUsize::new(1),
        ),
    };
    let mut builder = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "run-terminal"),
            [ScriptedTurn::completed(turn)],
        ))
        .tool(ScriptedTool::new(
            ToolSpec {
                name: "noop".into(),
                description: "no-op".into(),
                input_schema: json!({"type": "object"}),
            },
            ScriptedToolOutcome::Success(ToolOutput::text("ok")),
        ));
    if let Some(max_steps) = max_steps {
        builder = builder.max_steps(max_steps);
    }
    let runtime = builder.build().unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let outcome = session.complete("go").await.unwrap();
    runtime.shutdown();
    Ok(TurnOutcome::Completed(outcome))
}

/// The terminal, comparable as a whole.
#[derive(Debug, PartialEq)]
enum Terminal {
    Completed,
    MaxSteps,
    Timeout,
    Interrupted,
    Failed(String),
}

fn terminal(terminal: RunTerminal) -> Terminal {
    match terminal {
        RunTerminal::Completed(_) => Terminal::Completed,
        RunTerminal::MaxSteps(_) => Terminal::MaxSteps,
        RunTerminal::Timeout => Terminal::Timeout,
        RunTerminal::Interrupted(_) => Terminal::Interrupted,
        RunTerminal::Failed(error) => Terminal::Failed(error.to_string()),
    }
}

// Covers: a stop (timeout, signal) wins over however the prompt ended, the
// SDK's step-limit stop becomes max_steps without CLI flag context, and a
// cancel nothing in `rho run` asked for fails the run.
// Owner: pure unit (run terminal classification)
#[tokio::test]
async fn classifies_how_the_prompt_ended_and_what_stopped_it() {
    for (ended, stop, expected) in [
        (Ended::Answered, None, Terminal::Completed),
        (Ended::AtStepLimit, None, Terminal::MaxSteps),
        (Ended::Answered, Some(StopCause::Timeout), Terminal::Timeout),
        (
            Ended::Failed,
            Some(StopCause::Interrupted(ShutdownSignal::Interrupt)),
            Terminal::Interrupted,
        ),
        (
            Ended::Cancelled,
            None,
            Terminal::Failed("rho run was cancelled".into()),
        ),
        (
            Ended::Failed,
            None,
            Terminal::Failed("provider unavailable".into()),
        ),
    ] {
        let end = PromptEnd::of(prompt_that(ended).await, stop);
        assert_eq!(
            terminal(classify_run_terminal(end)),
            expected,
            "{ended:?} stopped by {stop:?}"
        );
    }
}

#[test]
fn prompt_requires_input() {
    let mut stdin = io::empty();
    let error = prompt_from_reader(Vec::new(), /*read_stdin*/ false, &mut stdin).unwrap_err();

    assert!(error.to_string().contains("requires a prompt"));
}
