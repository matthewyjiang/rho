//! What choosing a `/config` row does, as its picker value carries it,
//! and the permission-mode page the root and `/permissions` share.
//!
//! Every row of every settings page, the session rows, the local rows, the
//! daemon's settings, and the `/advisor on` models, is one [`ConfigRow`]
//! with one encoding, so choosing a row is one exhaustive match.

use rho_protocol::{SettingKey, SettingValue};
use serde::{Deserialize, Serialize};

use super::{
    daemon_settings::SettingsNamespace, PickerBadge, PickerBadgeTone, PickerItem, UiPicker,
};
use crate::permission::PermissionMode;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub(super) enum ConfigRow {
    /// Opens the session's model page.
    SessionModel,
    /// Cycles the session's reasoning level.
    SessionReasoning,
    /// Flips the session's fast mode.
    SessionFastMode,
    /// Opens the session's permission-mode page.
    SessionPermissionMode,
    /// Sets the session's permission mode.
    SetPermissionMode { mode: PermissionMode },
    /// Opens the theme picker, local to this terminal.
    Theme,
    /// Asks, then deletes every saved composer prompt.
    ClearPromptHistory,
    /// Opens category `id`'s page.
    Category {
        namespace: SettingsNamespace,
        id: String,
    },
    /// Changes setting `key`: a bool flips, other kinds open an editor.
    Setting {
        namespace: SettingsNamespace,
        key: SettingKey,
    },
    /// Writes `value` to `key`.
    Set {
        namespace: SettingsNamespace,
        key: SettingKey,
        value: SettingValue,
    },
    /// Removes `key` from the file.
    Reset {
        namespace: SettingsNamespace,
        key: SettingKey,
    },
    /// Turns the advisor on with `model`, in one write.
    EnableAdvisor { model: SettingValue },
}

const PREFIX: &str = "config:";

impl ConfigRow {
    pub(super) fn encode(&self) -> String {
        let json = serde_json::to_string(self).expect("config rows serialize");
        format!("{PREFIX}{json}")
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        serde_json::from_str(value.strip_prefix(PREFIX)?).ok()
    }
}

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
                    ConfigRow::SetPermissionMode { mode: candidate }.encode(),
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
