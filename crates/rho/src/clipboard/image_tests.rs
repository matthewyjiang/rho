use pretty_assertions::assert_eq;

use super::{
    available_image_helpers_with, platform_image_helpers, read_clipboard_image_for_session,
    select_preferred_image_mime_type, ClipboardImageError,
};
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
fn remote_sessions_expose_no_image_helpers() {
    let helpers = available_image_helpers_with(SessionKind::Remote, |_| true);
    assert!(helpers.is_empty());
}

#[test]
fn wsl_sessions_include_powershell_when_present() {
    let helpers = available_image_helpers_with(SessionKind::Wsl, |command| {
        matches!(command, "wl-paste" | "powershell.exe")
    });
    assert_eq!(helpers, vec!["wl-paste", "powershell.exe"]);
}

#[test]
fn local_sessions_report_platform_helpers() {
    let helpers = available_image_helpers_with(SessionKind::Local, |_| true);
    assert_eq!(helpers, platform_image_helpers());
}

#[test]
fn remote_sessions_do_not_read_host_image_clipboards() {
    let error = read_clipboard_image_for_session(SessionKind::Remote).unwrap_err();
    assert!(matches!(error, ClipboardImageError::NoImage));
}
