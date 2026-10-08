use pretty_assertions::assert_eq;

use super::super::{agent_picker::InternalAgentModelPickerOrigin, tests::test_app, ComposerMode};
use crate::permission::PermissionMode;

// Covers: cycling wraps in restrictiveness order and cannot enable Auto without
// a classifier, including when the current mode itself is unavailable Auto.
// Owner: permission cycle policy (pure unit)
#[test]
fn cycle_order_skips_unconfigured_auto() {
    use PermissionMode::{AllowEdits, Auto, Bypass, Plan, Supervised};

    for (classifier_configured, cases) in [
        (
            true,
            [
                (Plan, Supervised),
                (Supervised, AllowEdits),
                (AllowEdits, Auto),
                (Auto, Bypass),
                (Bypass, Plan),
            ],
        ),
        (
            false,
            [
                (Plan, Supervised),
                (Supervised, AllowEdits),
                (AllowEdits, Bypass),
                (Auto, Bypass),
                (Bypass, Plan),
            ],
        ),
    ] {
        for (current, expected) in cases {
            assert_eq!(
                super::next_cycle_mode(current, classifier_configured),
                expected,
                "current={current:?}, classifier_configured={classifier_configured}"
            );
        }
    }
}

// Covers: a failed model resolve (selected=false) must not schedule demote;
// only Esc cancel does. Otherwise Auto would flip to Supervised on a bad pick.
// Owner: permission mode startup gate
#[tokio::test]
async fn finish_startup_without_selection_does_not_mark_demote() {
    use crate::app::interactive_runtime::test_edit_tool_runtime;
    use crate::config::EditTool;

    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.pending_auto_classifier_demote = false;
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;

    app.finish_permission_classifier_model_selection(
        /*selected*/ false,
        InternalAgentModelPickerOrigin::PermissionModeStartup,
        &mut agent,
    )
    .await
    .unwrap();

    assert!(!app.pending_auto_classifier_demote);
    assert_eq!(app.info.runtime.permission_mode, PermissionMode::Auto);
}

// Covers: free composer + Auto without classifier must not stay silent —
// either open the startup picker or demote when no models exist.
// Owner: permission mode startup gate
#[tokio::test]
async fn reconcile_auto_without_classifier_opens_picker_or_demotes() {
    use crate::app::interactive_runtime::test_edit_tool_runtime;
    use crate::config::EditTool;

    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.info.runtime.internal_agents.clear();
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    agent
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();

    app.reconcile_auto_classifier_gate(&mut agent)
        .await
        .unwrap();

    let opened_startup_picker = matches!(
        app.internal_agent_model_target.as_ref().map(|t| t.origin),
        Some(InternalAgentModelPickerOrigin::PermissionModeStartup)
    ) && matches!(app.input_ui.composer(), ComposerMode::Picker(_));
    let demoted = app.info.runtime.permission_mode == PermissionMode::Supervised
        && agent.permission_mode() == PermissionMode::Supervised;
    assert!(
        opened_startup_picker || demoted,
        "expected startup picker or Supervised fallback, got mode={:?}",
        app.info.runtime.permission_mode,
    );
}

// Covers: pending demote from startup Esc is applied on the next idle reconcile.
// Owner: permission mode startup gate
#[tokio::test]
async fn reconcile_applies_pending_startup_demote() {
    use crate::app::interactive_runtime::test_edit_tool_runtime;
    use crate::config::EditTool;

    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.info.runtime.internal_agents.clear();
    app.pending_auto_classifier_demote = true;
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    agent
        .set_permission_mode(PermissionMode::Auto)
        .await
        .unwrap();

    app.reconcile_auto_classifier_gate(&mut agent)
        .await
        .unwrap();

    assert!(!app.pending_auto_classifier_demote);
    assert_eq!(app.info.runtime.permission_mode, PermissionMode::Supervised);
    assert_eq!(agent.permission_mode(), PermissionMode::Supervised);
}

// Covers: a classifier setting saved without the agent at hand, such as the
// screen allow threshold, reaches the runtime on the next apply, once.
// Owner: permission classifier config sync
#[tokio::test]
async fn pending_classifier_config_reaches_the_runtime_once() {
    use crate::app::interactive_runtime::test_edit_tool_runtime;
    use crate::config::{EditTool, InternalAgentModelConfig};
    use crate::permission_classifier::DECISION_SCREEN_ID;

    let mut app = test_app();
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    let mut screen = InternalAgentModelConfig::new(
        "typesafe".into(),
        "jev-latest".into(),
        "typesafe-api-key".into(),
    );
    screen.expect_rho_mut().allow_threshold_percent = Some(90);
    app.info
        .runtime
        .internal_agents
        .insert(DECISION_SCREEN_ID.into(), screen.clone());
    app.classifier_config_sync_pending = true;

    app.apply_pending_classifier_config(&mut agent);

    assert_eq!(
        agent
            .config_snapshot()
            .internal_agent_model(DECISION_SCREEN_ID)
            .cloned(),
        Some(screen)
    );
    assert!(!app.classifier_config_sync_pending);
}

// Covers: `/permissions` during compaction must not bubble `set_permission_mode`
// out of the event loop. Compact uses the idle composer, unlike a live turn.
// Owner: permission mode command
// PTY cannot hold an in-flight compact job deterministically, so this stays a
// command-layer unit test.
#[tokio::test]
async fn permissions_command_rejects_compaction_without_failing() {
    use crate::app::interactive_runtime::test_edit_tool_runtime;
    use crate::commands::parse_command;
    use crate::config::EditTool;

    let mut app = test_app();
    let mut agent = test_edit_tool_runtime(EditTool::Auto).await;
    let previous_agent = agent.permission_mode();
    let previous_ui = app.info.runtime.permission_mode;
    agent.begin_compact_task(/*instructions*/ None).unwrap();

    let invocation = parse_command("/permissions plan").unwrap().unwrap();
    app.execute_permissions_command(invocation, &mut agent)
        .await
        .unwrap();

    assert_eq!(agent.permission_mode(), previous_agent);
    assert_eq!(app.info.runtime.permission_mode, previous_ui);
    let _ = agent.abort_compact_task().await;
}
