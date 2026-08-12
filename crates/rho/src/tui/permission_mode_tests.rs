use pretty_assertions::assert_eq;

use super::super::{
    agent_picker::{InternalAgentModelPickerOrigin, InternalAgentModelTarget},
    tests::test_app,
    ComposerMode,
};
use crate::{agent::PERMISSION_CLASSIFIER_AGENT_ID, permission::PermissionMode};

fn startup_target() -> InternalAgentModelTarget {
    InternalAgentModelTarget {
        id: PERMISSION_CLASSIFIER_AGENT_ID.into(),
        origin: InternalAgentModelPickerOrigin::PermissionModeStartup,
    }
}

fn config_row_target() -> InternalAgentModelTarget {
    InternalAgentModelTarget {
        id: PERMISSION_CLASSIFIER_AGENT_ID.into(),
        origin: InternalAgentModelPickerOrigin::PermissionModeConfigRow,
    }
}

// Covers: Esc on the startup classifier picker must mark a demote so idle
// reconcile can apply Supervised without async agent handles on shared Esc.
// Owner: permission mode startup gate
#[test]
fn cancel_startup_classifier_prompt_marks_pending_demote() {
    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.internal_agent_model_target = Some(startup_target());

    assert!(app.cancel_permission_classifier_model_prompt(/*restore_input*/ true));

    assert!(app.pending_auto_classifier_demote);
    assert!(app.internal_agent_model_target.is_none());
    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert_eq!(
        app.status(),
        "permission mode set to supervised: no classifier model selected"
    );
}

// Covers: canceling Auto enable from config must not demote the current mode.
// Owner: permission mode startup gate
#[test]
fn cancel_config_row_classifier_prompt_keeps_mode_without_demote() {
    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Supervised;
    app.internal_agent_model_target = Some(config_row_target());

    assert!(app.cancel_permission_classifier_model_prompt(/*restore_input*/ true));

    assert!(!app.pending_auto_classifier_demote);
    assert_eq!(app.info.runtime.permission_mode, PermissionMode::Supervised);
    assert_eq!(
        app.status(),
        "permission mode stays supervised: no classifier model selected"
    );
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
