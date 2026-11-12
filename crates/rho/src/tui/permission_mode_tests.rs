use pretty_assertions::assert_eq;

use super::super::{agent_picker::InternalAgentModelPickerOrigin, tests::test_app, ComposerMode};
use crate::permission::PermissionMode;

// Covers: a failed model resolve (selected=false) must not schedule demote;
// only Esc cancel does. Otherwise Auto would flip to Supervised on a bad pick.
// Owner: permission mode startup gate
#[test]
fn finish_startup_without_selection_does_not_mark_demote() {
    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.pending_auto_classifier_demote = false;

    app.finish_permission_classifier_startup_selection(/*selected*/ false)
        .unwrap();

    assert!(!app.pending_auto_classifier_demote);
    assert_eq!(app.info.runtime.permission_mode, PermissionMode::Auto);
}

// Covers: free composer + Auto without classifier must not stay silent —
// either open the startup picker or save the Supervised fallback when no
// models exist.
// Owner: permission mode startup gate
#[test]
fn reconcile_auto_without_classifier_opens_picker_or_demotes() {
    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.info.runtime.internal_agents.clear();

    app.reconcile_auto_classifier_gate().unwrap();

    let opened_startup_picker = matches!(
        app.internal_agent_model_target.as_ref().map(|t| t.origin),
        Some(InternalAgentModelPickerOrigin::PermissionModeStartup)
    ) && matches!(app.input_ui.composer(), ComposerMode::Picker(_));
    let demoted = app
        .info
        .services
        .config_repository
        .load()
        .unwrap()
        .permission_mode
        == PermissionMode::Supervised;
    assert!(
        opened_startup_picker || demoted,
        "expected startup picker or Supervised fallback, got mode={:?}",
        app.info.runtime.permission_mode,
    );
}

// Covers: pending demote from startup Esc is saved on the next idle reconcile.
// Owner: permission mode startup gate
#[test]
fn reconcile_applies_pending_startup_demote() {
    let mut app = test_app();
    app.info.runtime.permission_mode = PermissionMode::Auto;
    app.info.runtime.internal_agents.clear();
    app.pending_auto_classifier_demote = true;

    app.reconcile_auto_classifier_gate().unwrap();

    assert!(!app.pending_auto_classifier_demote);
    assert_eq!(
        app.info
            .services
            .config_repository
            .load()
            .unwrap()
            .permission_mode,
        PermissionMode::Supervised
    );
}
