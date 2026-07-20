use pretty_assertions::assert_eq;

use super::{available_image_helpers, select_preferred_image_mime_type};
use crate::clipboard::SessionKind;

#[test]
fn selects_only_supported_image_mime_types() {
    assert_eq!(
        select_preferred_image_mime_type("image/tiff\nimage/jpeg"),
        Some("image/jpeg".into())
    );
    assert_eq!(select_preferred_image_mime_type("image/tiff"), None);
}

#[test]
fn image_helper_probe_returns_only_available_commands() {
    let helpers = available_image_helpers(SessionKind::Local);
    for helper in helpers {
        assert!(
            matches!(helper, "wl-paste" | "xclip" | "pngpaste" | "powershell"),
            "unexpected helper {helper}"
        );
    }
}
