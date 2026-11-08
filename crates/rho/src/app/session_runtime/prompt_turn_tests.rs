use std::{future::Future, path::Path, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    UserInput,
};
use tokio::sync::mpsc;

use crate::{
    app::session_runtime::{
        test_support::built_on, PlanReport, SessionEvent, SessionRuntime, SessionStorage, Turn,
        TurnControl, TurnIo, TurnOutcome, TurnView,
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

/// What a host was asked to show, in order.
#[derive(Debug, PartialEq)]
enum Hosted {
    Turn,
    Kept,
    Switched(PermissionMode),
}

/// A view that answers every plan approval with `answer` and logs what it
/// shows. With `cancel_before_handoff`, it cancels the prompt as it
/// finishes showing the first turn, before the handoff.
struct PlanView {
    answer: PlanPermissionAnswer,
    cancel_before_handoff: bool,
    hosted: Vec<Hosted>,
}

impl TurnView for PlanView {
    fn open_turn<'v>(
        &'v mut self,
        runtime: &SessionRuntime,
    ) -> (TurnIo, impl Future<Output = ()> + Send + use<'v>) {
        let (events, mut shown) = mpsc::unbounded_channel();
        let (controls, control) = mpsc::unbounded_channel();
        let answer = self.answer;
        let cancel =
            (self.cancel_before_handoff && self.hosted.is_empty()).then(|| runtime.handle());
        let hosted = &mut self.hosted;
        let showing = async move {
            while let Some(event) = shown.recv().await {
                if let SessionEvent::PlanApprovalRequested { id, .. } = event {
                    let _ = controls.send(TurnControl::AnswerPlan { id, answer });
                }
            }
            hosted.push(Hosted::Turn);
            if let Some(handle) = cancel {
                handle.cancel();
            }
        };
        (TurnIo { events, control }, showing)
    }

    fn report_plan(
        &mut self,
        runtime: &SessionRuntime,
        _process_config: &Config,
        report: PlanReport,
    ) -> impl Future<Output = ()> + Send {
        self.hosted.push(match report {
            PlanReport::Kept => Hosted::Kept,
            PlanReport::Switched => Hosted::Switched(runtime.permission_mode()),
        });
        std::future::ready(())
    }
}

fn ended_as(ended: &anyhow::Result<TurnOutcome>) -> &'static str {
    match ended {
        Ok(TurnOutcome::Completed(_)) => "completed",
        Ok(TurnOutcome::Cancelled) => "cancelled",
        Err(_) => "failed",
    }
}

// Covers: a prompt's turn and its plan handoff: no answered plan ends the
// prompt with the turn; a rejected plan or an unfinished turn keeps plan
// mode with a notice; a cancel before the switch keeps plan mode and ends
// the prompt cancelled; an approved plan on an answered turn switches modes,
// reports it, and then runs the implementation turn, whose end is the
// prompt's.
// Owner: session runtime prompt turns and plan handoff
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
            (vec![Hosted::Turn], "completed", PermissionMode::Plan),
        ),
        (
            "plan rejected",
            vec![propose_plan(), text("proposed")],
            PlanPermissionAnswer::KeepPlanning,
            false,
            (
                vec![Hosted::Turn, Hosted::Kept],
                "completed",
                PermissionMode::Plan,
            ),
        ),
        (
            // The scripted provider has no turn after the plan's answer.
            "turn failed after approval",
            vec![propose_plan()],
            approve,
            false,
            (
                vec![Hosted::Turn, Hosted::Kept],
                "failed",
                PermissionMode::Plan,
            ),
        ),
        (
            "cancelled before the switch",
            vec![propose_plan(), text("proposed")],
            approve,
            true,
            (
                vec![Hosted::Turn, Hosted::Kept],
                "cancelled",
                PermissionMode::Plan,
            ),
        ),
        (
            // The implementation turn takes the last scripted reply.
            "approved and answered",
            vec![propose_plan(), text("proposed"), text("implemented")],
            approve,
            false,
            (
                vec![
                    Hosted::Turn,
                    Hosted::Switched(PermissionMode::AllowEdits),
                    Hosted::Turn,
                ],
                "completed",
                PermissionMode::AllowEdits,
            ),
        ),
    ];
    for (label, turns, answer, cancel_before_handoff, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let mut runtime = plan_mode_runtime(root.path(), &config, turns).await;
        let mut view = PlanView {
            answer,
            cancel_before_handoff,
            hosted: Vec::new(),
        };
        let prompt = runtime.begin_prompt(&config).await.unwrap();
        let input = UserInput::text("plan it");
        let turn = Turn {
            display: input.clone(),
            model: input,
            calls: Vec::new(),
        };

        let ended = runtime
            .run_prompt_turn(&mut view, &prompt, turn, &config)
            .await;

        assert_eq!(
            (view.hosted, ended_as(&ended), runtime.permission_mode()),
            expected,
            "{label}"
        );
        drop(prompt);
        runtime.shutdown().await;
    }
}
