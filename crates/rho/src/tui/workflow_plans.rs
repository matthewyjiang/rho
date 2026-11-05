//! Saved frozen plans are secondary to sources and active runs in the hub.

use super::{
    item, short_id, App, ComposerMode, OverlayChrome, PickerBadgeTone, PickerLayout, UiPicker,
    PLAN_PREFIX, UNSUPPORTED_PLAN_PREFIX,
};
use crate::workflow::PlanInventory;

fn plans_picker(plans: &PlanInventory) -> UiPicker {
    let mut items = vec![item(
        None,
        "Back to workflows",
        "Start from source or inspect a run.",
        "browse:workflows",
        None,
        Some("back"),
    )];
    for plan in &plans.items {
        let id = plan.plan_id.to_string();
        let short = short_id(&id);
        let name = &plan.name;
        let steps = plan.step_count;
        items.push(item(
            Some("SAVED PLANS"),
            format!("{name} · {short}"),
            format!("{name}\n{steps} steps frozen\nEnter starts a new run. Press d to delete. Existing runs keep their own graph copy.\nPlan id {short}"),
            format!("{PLAN_PREFIX}{id}"),
            Some(("saved".into(), PickerBadgeTone::Internal)),
            Some("run"),
        ));
    }
    // Saved before scoped workflow programs: listed so they can be deleted.
    for record in &plans.unsupported {
        let id = record.id.to_string();
        let short = short_id(&id);
        let name = &record.name;
        items.push(item(
            Some("SAVED PLANS"),
            format!("{name} · {short}"),
            format!("{name}\nSaved by Rho before 2.15. Inspect it with Rho 2.x. Press d to delete.\nPlan id {short}"),
            format!("{UNSUPPORTED_PLAN_PREFIX}{id}"),
            Some(("unsupported".into(), PickerBadgeTone::Warning)),
            Some("close"),
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
