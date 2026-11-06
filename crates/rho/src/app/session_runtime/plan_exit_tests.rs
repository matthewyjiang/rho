use std::{path::Path, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    UserInput,
};
use tokio::sync::mpsc;

use super::{PlanHandoff, IMPLEMENT_PLAN};
use crate::{
    app::session_runtime::{
        test_support::built_on, SessionEvent, SessionRuntime, SessionStorage, Turn, TurnControl,
        TurnIo, TurnOutcome,
    },
    config::Config,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::{
        plan_exit::{PlanExitHost, PlanPermissionAnswer},
        sdk_registry::AppToolSet,
    },
};

fn text(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

fn propose_plan() -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall::new(
            "plan".into(),
            "exit_plan_mode".into(),
            serde_json::json!({"plan": "1. do it"}),
        ),
    )]))
}

/// A plan-mode runtime with `exit_plan_mode` on `turns`, whose plan prompts
/// go to the host as permission requests.
async fn plan_mode_runtime(
    root: &Path,
    config: &Config,
    turns: Vec<ScriptedTurn>,
) -> SessionRuntime {
    let stored = StoredSession::create_in_root(root, root).unwrap();
    let id = rho_sdk::SessionId::from_string(stored.id()).unwrap();
    let provider = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        turns,
    ));
    let mut built = built_on(config, id, provider).await;
    built.tools = AppToolSet::disabled().with_plan_exit_host(config, PlanExitHost::Permission);
    let mut runtime = SessionRuntime::for_tests(built, SessionStorage::saved(stored), config);
    // Entering plan mode rebuilds the SDK runtime with the tools, as a
    // host's mode change does.
    runtime
        .set_permission_mode(PermissionMode::Plan, config)
        .await
        .unwrap();
    runtime
}

/// What a handoff came to, without the run's outcome or the notice's copy.
#[derive(Debug, PartialEq)]
enum Handed {
    Ended {
        notice: bool,
        ended: &'static str,
    },
    Switched {
        mode: PermissionMode,
        implementation: Vec<ContentBlock>,
    },
}

fn ended_as(ended: &anyhow::Result<TurnOutcome>) -> &'static str {
    match ended {
        Ok(TurnOutcome::Completed(_)) => "completed",
        Ok(TurnOutcome::Cancelled) => "cancelled",
        Err(_) => "failed",
    }
}

// Covers: the plan handoff policy after the proposing turn: no answered plan
// leaves the prompt's end untouched; a rejected plan or an unfinished turn
// keeps plan mode with a notice; a cancel before the switch keeps plan mode
// and ends the prompt cancelled; an approved plan on an answered turn
// switches modes and asks for the implementation turn.
// Owner: session runtime plan handoff
#[tokio::test]
async fn the_plan_handoff_follows_the_decision_and_the_turn() {
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    };
    let approve = PlanPermissionAnswer::Approve(PermissionMode::AllowEdits);
    let cases = [
        (
            "no plan answered",
            vec![text("done")],
            approve,
            /*cancel_before_handoff*/ false,
            Handed::Ended {
                notice: false,
                ended: "completed",
            },
            PermissionMode::Plan,
        ),
        (
            "plan rejected",
            vec![propose_plan(), text("proposed")],
            PlanPermissionAnswer::KeepPlanning,
            false,
            Handed::Ended {
                notice: true,
                ended: "completed",
            },
            PermissionMode::Plan,
        ),
        (
            // The scripted provider has no turn after the plan's answer.
            "turn failed after approval",
            vec![propose_plan()],
            approve,
            false,
            Handed::Ended {
                notice: true,
                ended: "failed",
            },
            PermissionMode::Plan,
        ),
        (
            "cancelled before the switch",
            vec![propose_plan(), text("proposed")],
            approve,
            true,
            Handed::Ended {
                notice: true,
                ended: "cancelled",
            },
            PermissionMode::Plan,
        ),
        (
            "approved and answered",
            vec![propose_plan(), text("proposed")],
            approve,
            false,
            Handed::Switched {
                mode: PermissionMode::AllowEdits,
                implementation: UserInput::text(IMPLEMENT_PLAN).blocks().to_vec(),
            },
            PermissionMode::AllowEdits,
        ),
    ];
    for (label, turns, answer, cancel_before_handoff, expected, mode_after) in cases {
        let root = tempfile::tempdir().unwrap();
        let mut runtime = plan_mode_runtime(root.path(), &config, turns).await;
        let prompt = runtime.begin_prompt(&config).await.unwrap();
        let (events, mut shown) = mpsc::unbounded_channel();
        let (controls, control) = mpsc::unbounded_channel();
        let answer_plans = async move {
            while let Some(event) = shown.recv().await {
                if let SessionEvent::PlanApprovalRequested { id, .. } = event {
                    let _ = controls.send(TurnControl::AnswerPlan { id, answer });
                }
            }
        };
        let input = UserInput::text("plan it");
        let turn = Turn {
            display: input.clone(),
            model: input,
            calls: Vec::new(),
        };
        let (proposal, ()) = tokio::join!(
            runtime.run_turn(&prompt, turn, TurnIo { events, control }, &config),
            answer_plans,
        );
        if cancel_before_handoff {
            runtime.handle().cancel();
        }

        let handoff = runtime.hand_off_plan(&prompt, proposal, &config).await;

        let handed = match handoff {
            PlanHandoff::Ended { notice, ended } => Handed::Ended {
                notice: notice.is_some(),
                ended: ended_as(&ended),
            },
            PlanHandoff::Switched {
                mode,
                notice: _,
                implementation,
            } => Handed::Switched {
                mode,
                implementation: implementation.model.blocks().to_vec(),
            },
        };
        assert_eq!(
            (handed, runtime.permission_mode()),
            (expected, mode_after),
            "{label}"
        );
        drop(prompt);
        runtime.shutdown().await;
    }
}
