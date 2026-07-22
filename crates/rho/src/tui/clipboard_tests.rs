use super::super::tests::test_app;

#[test]
fn image_paste_is_unavailable_while_running() {
    let mut app = test_app();
    app.running = true;

    app.paste_clipboard_image();

    assert!(app.pending_images.is_empty());
    assert_eq!(
        app.status,
        "image paste is unavailable while a model turn is running"
    );
}

#[test]
fn single_line_image_path_paste_attaches_image_instead_of_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.png");
    let png = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    )
    .unwrap();
    std::fs::write(&path, png).unwrap();

    let mut app = test_app();
    app.info.runtime.cwd = dir.path().to_path_buf();
    app.insert_paste(&path.to_string_lossy());

    assert_eq!(app.pending_images.len(), 1);
    assert_eq!(app.pending_images[0].mime_type, "image/png");
    assert!(app.input.is_empty());
    assert!(app.status.starts_with("attached image 1"));
}

#[test]
fn non_image_path_paste_stays_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello").unwrap();

    let mut app = test_app();
    app.info.runtime.cwd = dir.path().to_path_buf();
    app.insert_paste(&path.to_string_lossy());

    assert!(app.pending_images.is_empty());
    assert!(app.input.contains("notes.txt") || !app.paste_segments.is_empty());
}

#[cfg(unix)]
#[test]
fn unreadable_image_path_paste_reports_error_without_inserting_text() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.png");
    let png = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    )
    .unwrap();
    std::fs::write(&path, png).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut app = test_app();
    app.info.runtime.cwd = dir.path().to_path_buf();
    app.insert_paste(&path.to_string_lossy());

    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

    assert!(app.pending_images.is_empty());
    assert!(app.input.is_empty());
    assert!(app.status.contains("image paste failed"));
}
