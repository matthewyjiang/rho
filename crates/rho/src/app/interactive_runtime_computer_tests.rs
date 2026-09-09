use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelResponse},
    provider::ScriptedTurn,
    UserInput,
};

use crate::tools::computer_use::{ComputerUseSession, ComputerUseStatus};

// Covers: eligibility can change after a consent prompt; grant and installation
// must recheck the same typed runtime policy before starting any driver process.
#[tokio::test]
async fn computer_grants_recheck_runtime_eligibility() {
    use super::ComputerUseEligibilityError;
    use crate::permission::PermissionMode;

    for rejection in [
        ComputerUseEligibilityError::UnsupportedHost,
        ComputerUseEligibilityError::PlanMode,
        ComputerUseEligibilityError::Busy,
    ] {
        let mut runtime = super::super::tests::test_runtime(vec![ScriptedTurn::completed(
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )])
        .await;
        let root = tempfile::tempdir().unwrap();
        if rejection != ComputerUseEligibilityError::UnsupportedHost {
            runtime.tools = runtime.tools.with_computer_use(ComputerUseSession::new(
                Some(root.path().join("missing-driver")),
                crate::config::Config::default().max_output_bytes,
                root.path().into(),
            ));
            assert!(runtime.computer_use_eligibility().is_ok());
        }
        match rejection {
            ComputerUseEligibilityError::UnsupportedHost => {}
            ComputerUseEligibilityError::PlanMode => runtime.permission_mode = PermissionMode::Plan,
            ComputerUseEligibilityError::Busy => {
                runtime
                    .start(UserInput::text("busy"), /*display_user*/ None)
                    .await
                    .unwrap();
            }
        }
        assert_eq!(runtime.computer_use_eligibility().err(), Some(rejection));
        for error in [
            runtime.enable_computer_use().unwrap_err(),
            runtime.install_computer_driver().unwrap_err(),
        ] {
            assert_eq!(
                error.downcast_ref::<ComputerUseEligibilityError>(),
                Some(&rejection)
            );
        }
        if rejection == ComputerUseEligibilityError::Busy {
            while runtime.next_event().await.is_some() {}
            runtime.finish_run().await.unwrap();
        }
    }
}

// Covers: revocation before a pending session replacement must not rebuild the
// old runtime, but an unreplaced live session must bind the removal before use.
// Owner: runtime lifecycle. Exercise the reset controller without global consent I/O.
#[tokio::test]
async fn computer_revocation_defers_rebind_only_for_pending_replacement() {
    for pending_replacement in [false, true] {
        let mut runtime = super::super::tests::test_runtime(vec![ScriptedTurn::completed(
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )])
        .await;
        let root = tempfile::tempdir().unwrap();
        runtime.tools = runtime.tools.with_computer_use(ComputerUseSession::new(
            None,
            crate::config::Config::default().max_output_bytes,
            root.path().into(),
        ));
        // Model a registered handle whose grant has just been revoked.
        runtime.tools.set_computer_use_registered(true);
        runtime.rebind_current_session().await.unwrap();
        let previous = runtime.runtime.clone();
        runtime.revoke_computer_use();
        assert!(!runtime.tools.contains("computer"));
        assert!(runtime.computer_runtime_dirty);
        if pending_replacement {
            runtime.sessions.reset().unwrap();
        }
        runtime.reconcile_computer_use().await.unwrap();
        assert_eq!(runtime.computer_runtime_dirty, pending_replacement);
        assert_eq!(
            previous
                .session(rho_sdk::SessionOptions::default())
                .await
                .is_ok(),
            pending_replacement,
        );
        runtime
            .start(UserInput::text("next"), /*display_user*/ None)
            .await
            .unwrap();
        while runtime.next_event().await.is_some() {}
        runtime.finish_run().await.unwrap();
        assert!(!runtime.computer_runtime_dirty);
        assert!(!runtime.tools.contains("computer"));
        assert!(previous
            .session(rho_sdk::SessionOptions::default())
            .await
            .is_err());
    }
}

// Covers: start_run consuming a completed driver failure must not discard input.
// Owner: interactive runtime boundary. PTY polling usually consumes the failure
// first, so hold the completed connection result until this exact boundary.
#[tokio::test]
async fn failed_computer_connect_preserves_next_prompt() {
    let mut runtime = super::super::tests::test_runtime(vec![ScriptedTurn::completed(
        ModelResponse::Assistant(vec![ContentBlock::Text("prompt received".into())]),
    )])
    .await;
    let root = tempfile::tempdir().unwrap();
    let computer = ComputerUseSession::new(
        Some(root.path().join("missing-driver")),
        crate::config::Config::default().max_output_bytes,
        root.path().into(),
    );
    runtime.tools = runtime.tools.with_computer_use(computer.clone());
    runtime.enable_computer_use().unwrap();
    computer.wait_for_connect_result().await;
    runtime
        .start(
            UserInput::text("keep my prompt"),
            /*display_user*/ None,
        )
        .await
        .expect("driver failures must not abort a turn");
    while runtime.next_event().await.is_some() {}
    runtime.finish_run().await.unwrap();
    assert_eq!(computer.status(), ComputerUseStatus::Off);
    assert!(!runtime.tools.contains("computer"));
    assert_eq!(
        runtime
            .history()
            .iter()
            .find(|message| **message == Message::user_text("keep my prompt")),
        Some(&Message::user_text("keep my prompt"))
    );
    assert_eq!(runtime.take_notices().len(), 1);
    assert!(runtime.take_notices().is_empty());
}
