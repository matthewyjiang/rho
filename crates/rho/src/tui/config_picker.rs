//! Row values and the permission-mode page the daemon `/config` root and
//! the `/permissions` picker share.

use super::{PickerBadge, PickerBadgeTone, PickerItem, UiPicker};
use crate::permission::PermissionMode;

pub(super) const CONVERSATION_MODEL_VALUE: &str = "conversation_model";
pub(super) const PERMISSION_MODE_VALUE: &str = "permission_mode";
pub(super) const PERMISSION_MODE_PREFIX: &str = "permission_mode:";
pub(super) const REASONING_VALUE: &str = "reasoning";
pub(super) const THEME_VALUE: &str = "theme";
pub(super) const CLEAR_PROMPT_HISTORY_VALUE: &str = "clear_prompt_history";

pub(super) fn permission_mode_picker(mode: PermissionMode) -> UiPicker {
    UiPicker::config(
        "Permission mode",
        PermissionMode::ALL
            .into_iter()
            .map(|candidate| PickerItem {
                detail: Some(permission_mode_description(candidate).into()),
                badge: (candidate == mode).then_some(PickerBadge {
                    text: "selected".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                ..PickerItem::new(
                    candidate.label().into(),
                    format!("{PERMISSION_MODE_PREFIX}{}", candidate.as_str()),
                )
            })
            .collect(),
    )
}

fn permission_mode_description(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => "No permission checks.",
        PermissionMode::Auto => {
            "Classifier reviews new files, processes, and outside-workspace reads; tracked and already-approved workspace edits are free."
        }
        PermissionMode::AllowEdits => {
            "Tracked and already-approved workspace edits are free; ask before new files, processes, and outside-workspace reads."
        }
        PermissionMode::Plan => {
            "Investigate the workspace; writes, processes, and outside-workspace reads are denied."
        }
        PermissionMode::Supervised => {
            "Ask before writes, processes, and outside-workspace reads."
        }
    }
}
