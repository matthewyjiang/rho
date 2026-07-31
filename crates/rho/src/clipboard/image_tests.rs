use std::{fs, path::PathBuf};

use pretty_assertions::assert_eq;

use super::{
    available_image_helpers_with, image_content_from_bytes, paste_text_as_file_path,
    read_clipboard_image_for_session, read_image_file_with_limit, select_preferred_image_mime_type,
    ClipboardImageError,
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
fn remote_sessions_do_not_read_host_image_clipboards() {
    let error = read_clipboard_image_for_session(SessionKind::Remote).unwrap_err();
    assert!(matches!(error, ClipboardImageError::NoImage));
}

#[test]
fn paste_text_recognizes_supported_file_path_forms() {
    let (_dir, path) = write_temp_png();
    let cwd = path.parent().unwrap();

    assert_eq!(
        paste_text_as_file_path(&path.to_string_lossy(), cwd),
        Some(path.clone())
    );
    assert_eq!(paste_text_as_file_path("shot.png", cwd), Some(path.clone()));
    assert_eq!(
        paste_text_as_file_path(&format!("\"{}\"", path.display()), cwd),
        Some(path.clone())
    );
    assert_eq!(paste_text_as_file_path("shot.png\nextra", cwd), None);
    assert_eq!(paste_text_as_file_path("missing.txt", cwd), None);
    assert_eq!(paste_text_as_file_path("/", cwd), None);

    let unsupported_path = cwd.join("archive.bin");
    fs::write(&unsupported_path, [0, 1, 2, 3]).unwrap();
    assert_eq!(paste_text_as_file_path("archive.bin", cwd), None);

    let text_path = cwd.join("notes.txt");
    fs::write(&text_path, "hello").unwrap();
    assert_eq!(paste_text_as_file_path("notes.txt", cwd), Some(text_path));

    let spaced_path = cwd.join("drop report.txt");
    fs::write(&spaced_path, "dropped").unwrap();
    assert_eq!(
        paste_text_as_file_path(&spaced_path.to_string_lossy(), cwd),
        Some(spaced_path.clone())
    );
    let file_url = url::Url::from_file_path(&spaced_path).unwrap();
    assert_eq!(
        paste_text_as_file_path(file_url.as_str(), cwd),
        Some(spaced_path.clone())
    );
    #[cfg(unix)]
    assert_eq!(
        paste_text_as_file_path(&spaced_path.to_string_lossy().replace(' ', "\\ "), cwd),
        Some(spaced_path)
    );

    let colon_dir = cwd.join("drop:");
    fs::create_dir(&colon_dir).unwrap();
    let colon_path = colon_dir.join("report.txt");
    fs::write(&colon_path, "dropped").unwrap();
    let colon_input = format!("{}/drop://report.txt", cwd.display());
    assert_eq!(paste_text_as_file_path(&colon_input, cwd), Some(colon_path));
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
