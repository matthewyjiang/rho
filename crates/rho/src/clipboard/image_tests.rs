use std::{fs, path::PathBuf};

use pretty_assertions::assert_eq;

use super::{
    available_image_helpers_with, image_content_from_bytes, image_from_paste_text,
    paste_text_as_image_path, platform_image_helpers, read_clipboard_image_for_session,
    read_image_file, read_image_file_with_limit, select_preferred_image_mime_type,
    ClipboardImageError, PasteImageOutcome,
};
use crate::clipboard::SessionKind;

fn write_temp_png() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    // 1x1 transparent PNG
    let png = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    )
    .unwrap();
    fs::write(&path, png).unwrap();
    (dir, path)
}

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

#[test]
fn paste_text_recognizes_absolute_and_relative_image_paths() {
    let (_dir, path) = write_temp_png();
    let cwd = path.parent().unwrap();

    assert_eq!(
        paste_text_as_image_path(&path.to_string_lossy(), cwd),
        Some(path.canonicalize().unwrap())
    );
    assert_eq!(
        paste_text_as_image_path("shot.png", cwd),
        Some(path.canonicalize().unwrap())
    );
    assert_eq!(
        paste_text_as_image_path(&format!("\"{}\"", path.display()), cwd),
        Some(path.canonicalize().unwrap())
    );
    assert_eq!(paste_text_as_image_path("shot.png\nextra", cwd), None);

    let text_path = cwd.join("notes.txt");
    fs::write(&text_path, "hello").unwrap();
    assert_eq!(
        paste_text_as_image_path("notes.txt", cwd),
        Some(text_path.canonicalize().unwrap())
    );
    assert!(matches!(
        image_from_paste_text("notes.txt", cwd),
        PasteImageOutcome::NotImage
    ));
    assert!(matches!(
        image_from_paste_text("shot.png", cwd),
        PasteImageOutcome::Image(_)
    ));
}

#[test]
fn read_image_file_loads_supported_bytes() {
    let (_dir, path) = write_temp_png();
    let image = read_image_file(&path).unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert!(!image.data.is_empty());
}

#[test]
fn image_content_rejects_non_image_bytes() {
    let error = image_content_from_bytes(b"hello".to_vec()).unwrap_err();
    assert!(matches!(error, ClipboardImageError::NoImage));
}

#[test]
fn read_image_file_rejects_oversized_payload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.png");
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&[0_u8; 16]);
    fs::write(&path, &bytes).unwrap();

    let error = read_image_file_with_limit(&path, 8).unwrap_err();
    assert!(matches!(error, ClipboardImageError::TooLarge(8)));
}
