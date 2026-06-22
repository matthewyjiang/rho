use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, TuiInfo, UiPicker};
pub(super) const REASONING_VALUE: &str = "reasoning";
pub(super) const MAX_OUTPUT_BYTES_VALUE: &str = "max_output_bytes";

pub(super) fn config_picker(info: &TuiInfo, max_output_bytes: usize) -> UiPicker {
    UiPicker::new(
        "Config",
        "type regex filter, enter change, esc cancel",
        vec![
            PickerItem {
                label: "Reasoning".into(),
                detail: Some(format!(
                    "Controls model reasoning. Current: {}; Enter cycles to {}.",
                    info.reasoning,
                    info.reasoning.next()
                )),
                preview: None,
                badge: Some(PickerBadge {
                    text: info.reasoning.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: REASONING_VALUE.into(),
            },
            PickerItem {
                label: "Max output bytes".into(),
                detail: Some(
                    "Maximum tool output retained in context. Saved for next session.".into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: max_output_bytes.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: MAX_OUTPUT_BYTES_VALUE.into(),
            },
        ],
        PickerAction::Config,
    )
}
