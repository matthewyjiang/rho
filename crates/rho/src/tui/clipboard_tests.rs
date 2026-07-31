use super::{super::tests::test_app, ChatMedia, ChatTextDocument};

async fn insert_external_paste_and_finish(app: &mut super::App, text: &str) {
    app.insert_external_paste(text);
    if !app.pending_media_attaches.is_empty() {
        let outcome = super::next_pending_media_attach(&mut app.pending_media_attaches).await;
        app.finish_pasted_media(outcome);
    }
}

#[test]
fn image_paste_is_unavailable_while_running() {
    let mut app = test_app();
    app.begin_provider_turn_ui();

    app.paste_clipboard_image();

    assert!(app.input_ui.pending_media().is_empty());
}

#[tokio::test]
async fn pending_media_poll_is_cancellation_safe() {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let mut pending = std::collections::VecDeque::from([super::PendingMediaAttach {
        task: tokio::spawn(async {
            let _ = release_rx.await;
            super::PastedMediaOutcome::Unsupported {
                original_text: "archive.bin".into(),
            }
        }),
    }]);

    let mut first_poll = Box::pin(super::next_pending_media_attach(&mut pending));
    assert!(futures_util::poll!(&mut first_poll).is_pending());
    drop(first_poll);
    assert_eq!(pending.len(), 1);

    let _ = release_tx.send(());
    let outcome = super::next_pending_media_attach(&mut pending).await;
    assert!(matches!(
        outcome,
        super::PastedMediaOutcome::Unsupported { original_text }
            if original_text == "archive.bin"
    ));
    assert!(pending.is_empty());
}

#[tokio::test]
async fn single_line_image_path_paste_attaches_image_instead_of_text() {
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
    insert_external_paste_and_finish(&mut app, &path.to_string_lossy()).await;

    assert_eq!(app.input_ui.pending_media().len(), 1);
    assert!(matches!(
        &app.input_ui.pending_media()[0],
        ChatMedia::Image(image) if image.mime_type == "image/png"
    ));
    assert!(app.input_ui.text().is_empty());
}

#[tokio::test]
async fn text_document_path_paste_attaches_document_instead_of_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "hello").unwrap();

    let mut app = test_app();
    app.info.runtime.cwd = dir.path().to_path_buf();
    insert_external_paste_and_finish(&mut app, &path.to_string_lossy()).await;

    assert_eq!(
        app.input_ui.pending_media(),
        &[ChatMedia::TextDocument(ChatTextDocument {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            body: "hello".into(),
            truncated: false,
            warnings: Vec::new(),
        })]
    );
    assert!(app.input_ui.text().is_empty());
}

#[tokio::test]
async fn unsupported_binary_path_paste_stays_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("archive.bin");
    std::fs::write(&path, [0, 1, 2, 3]).unwrap();

    let mut app = test_app();
    app.info.runtime.cwd = dir.path().to_path_buf();
    insert_external_paste_and_finish(&mut app, &path.to_string_lossy()).await;

    assert!(app.input_ui.pending_media().is_empty());
    assert!(!app.input_ui.text().is_empty() || !app.input_ui.paste_segments().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_image_path_paste_reports_error_without_inserting_text() {
    use std::os::unix::fs::PermissionsExt;

    // SAFETY: `geteuid` takes no pointers and has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.png");
    let png = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    )
    .unwrap();
    std::fs::write(&path, png).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root or CAP_DAC_OVERRIDE can still open 0o000 files; skip if this process can.
    if std::fs::File::open(&path).is_ok() {
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        return;
    }

    let mut app = test_app();
    app.info.runtime.cwd = dir.path().to_path_buf();
    insert_external_paste_and_finish(&mut app, &path.to_string_lossy()).await;

    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

    assert!(app.input_ui.pending_media().is_empty());
    assert!(app.input_ui.text().is_empty());
}
