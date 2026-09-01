//! `/doctor` picker adapter. Check policy lives in `crate::doctor`; this
//! module only maps report rows onto picker items.

use super::{
    picker::OverlayChrome, PickerBadge, PickerBadgePlacement, PickerBadgeTone, PickerItem,
    PickerLayout, UiPicker,
};
use crate::doctor::{DoctorCheck, DoctorProbeGate, DoctorReport, DoctorStatus};

/// Live probes stay out of unit tests, mirroring `/limits`.
pub(super) fn probe_gate() -> DoctorProbeGate {
    if cfg!(test) {
        DoctorProbeGate::Disabled
    } else {
        DoctorProbeGate::Live
    }
}

pub(super) fn picker(report: &DoctorReport) -> UiPicker {
    let items = report
        .sections
        .iter()
        .flat_map(|section| {
            section
                .checks
                .iter()
                .map(|check| picker_item(section.id.label(), check))
        })
        .collect();
    UiPicker::dismiss("Doctor diagnostics", items)
        .with_layout(PickerLayout::Overlay)
        .with_badge_placement(PickerBadgePlacement::Detail)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " CHECKS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ checks".into(),
        })
        .with_confirm_verb("close")
}

fn picker_item(section: &str, check: &DoctorCheck) -> PickerItem {
    PickerItem {
        value: check.label.clone(),
        label: check.label.clone(),
        section: Some(section.to_ascii_uppercase()),
        detail: Some(check.hint.clone().unwrap_or_else(|| check.summary.clone())),
        preview: None,
        badge: Some(PickerBadge {
            text: check.summary.clone(),
            tone: badge_tone(check.status),
        }),
        selection_verb: None,
        allow_filter_completion: true,
    }
}

fn badge_tone(status: DoctorStatus) -> PickerBadgeTone {
    match status {
        DoctorStatus::Ok | DoctorStatus::Info => PickerBadgeTone::Healthy,
        DoctorStatus::Warn | DoctorStatus::Fail | DoctorStatus::Checking => {
            PickerBadgeTone::Warning
        }
    }
}
