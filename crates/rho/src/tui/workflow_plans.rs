//! Saved frozen plans are secondary to sources and active runs in the hub.

use super::{
    item, short_id, App, ComposerMode, OverlayChrome, PickerBadgeTone, PickerLayout,
    PlanInventoryItem, RecordAccess, UiPicker, PLAN_PREFIX, READ_ONLY_PLAN_PREFIX,
};

fn plans_picker(plans: &[PlanInventoryItem]) -> UiPicker {
    let mut items = vec![item(
        None,
        "Back to workflows",
        "Start from source or inspect a run.",
        "browse:workflows",
        None,
        Some("back"),
    )];
    for plan in plans {
        let id = plan.plan_id.to_string();
        let short = short_id(&id);
        let name = &plan.name;
        let steps = plan.step_count;
        let (prefix, status, action, detail) = match plan.access {
            RecordAccess::ReadOnly => (
                READ_ONLY_PLAN_PREFIX,
                "read-only",
                "close",
                "Read-only legacy plan. Create a new plan from source to run it. Press d to delete.",
            ),
            RecordAccess::Executable => (
                PLAN_PREFIX,
                "saved",
                "run",
                "Enter starts a new run. Press d to delete. Existing runs keep their own graph copy.",
            ),
        };
        items.push(item(
            Some("SAVED PLANS"),
            format!("{name} · {short}"),
            format!("{name}\n{steps} steps frozen\n{detail}\nPlan id {short}"),
            format!("{prefix}{id}"),
            Some((status.into(), PickerBadgeTone::Internal)),
            Some(action),
        ));
    }
    UiPicker::workflow("Saved plans", items)
        .with_key_hints(crate::tui::PickerKeyHints {
            tab: crate::tui::TabKey::None,
            row_delete: crate::tui::RowDeleteKeys::DOrDelete,
            ..Default::default()
        })
        .with_layout(PickerLayout::Overlay)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SAVED PLANS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ items".into(),
        })
        .with_confirm_verb("open")
}

impl App {
    pub(super) fn open_workflow_plans(&mut self) -> anyhow::Result<()> {
        let plans = self.workflow_ops()?.list_workspace_plans()?;
        self.input_ui
            .set_composer(ComposerMode::Picker(plans_picker(&plans)));
        self.set_status("saved plans");
        Ok(())
    }
}
