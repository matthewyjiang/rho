use super::{
    super::{tests::test_app, ComposerAttachment},
    ChatMedia, ChatTextDocument, ImageContent, MediaAttachId,
};

async fn insert_external_paste_and_finish(app: &mut super::App, text: &str) {
    app.insert_external_paste(text);
    if !app.media_attach_tasks.is_empty() {
        let outcome = super::next_media_attach_completion(&mut app.media_attach_tasks).await;
        app.finish_pasted_media(outcome);
    }
}

#[test]
fn image_paste_is_unavailable_while_running() {
    let mut app = test_app();
    app.begin_provider_turn_ui();

    app.paste_clipboard_image();

    assert!(app.input_ui.attachments().is_empty());
}

#[tokio::test]
async fn attachment_task_poll_is_cancellation_safe() {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async {
        let _ = release_rx.await;
        super::PastedMediaOutcome::Unsupported {
            original_text: "archive.bin".into(),
        }
    });
    let id = MediaAttachId::new();
    let mut pending = vec![super::MediaAttachTask { id, task }];

    let mut first_poll = Box::pin(super::next_media_attach_completion(&mut pending));
    assert!(futures_util::poll!(&mut first_poll).is_pending());
    drop(first_poll);
    assert_eq!(pending.len(), 1);

    let _ = release_tx.send(());
    let completion = super::next_media_attach_completion(&mut pending).await;
    assert_eq!(completion.id, id);
    assert!(matches!(
        completion.outcome,
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

    assert_eq!(app.input_ui.attachments().len(), 1);
    assert!(matches!(
        &app.input_ui.attachments()[0],
        ComposerAttachment::Ready(ChatMedia::Image(image)) if image.mime_type == "image/png"
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
    app.insert_external_paste(&path.to_string_lossy());

    assert_eq!(
        app.input_ui.attachments(),
        &[ComposerAttachment::Pending {
            id: app.media_attach_tasks[0].id,
            name: "notes.txt".into(),
        }]
    );

    let outcome = super::next_media_attach_completion(&mut app.media_attach_tasks).await;
    app.finish_pasted_media(outcome);

    assert_eq!(
        app.input_ui.attachments(),
        &[ComposerAttachment::Ready(ChatMedia::TextDocument(
            ChatTextDocument {
                name: "notes.txt".into(),
                mime: "text/plain".into(),
                body: "hello".into(),
                truncated: false,
                warnings: Vec::new(),
            }
        ))]
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

    assert!(app.input_ui.attachments().is_empty());
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

    assert!(app.input_ui.attachments().is_empty());
    assert!(app.input_ui.text().is_empty());
}

// Covers: Backspace removes the last visible attachment instead of an earlier extraction task.
// Owner: TUI composer attachment orchestration.
#[tokio::test]
async fn backspace_removes_ready_image_after_pending_document() {
    let (mut release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async {
        let _ = release_rx.await;
        super::PastedMediaOutcome::Unsupported {
            original_text: "notes.txt".into(),
        }
    });
    let id = MediaAttachId::new();
    let mut app = test_app();
    app.media_attach_tasks
        .push(super::MediaAttachTask { id, task });
    app.input_ui.push_pending_attachment(id, "notes.txt".into());
    app.attach_ready_image(ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    });

    app.backspace_input();

    assert_eq!(
        app.input_ui.attachments(),
        &[ComposerAttachment::Pending {
            id,
            name: "notes.txt".into(),
        }]
    );
    assert_eq!(app.media_attach_tasks.len(), 1);
    assert_eq!(app.media_attach_tasks[0].id, id);

    app.backspace_input();

    assert!(app.input_ui.attachments().is_empty());
    assert!(app.media_attach_tasks.is_empty());
    tokio::time::timeout(std::time::Duration::from_secs(1), release_tx.closed())
        .await
        .expect("cancelled attachment task should drop its receiver");
}

// Covers: out-of-order completion replaces only its ID and pending items cannot become model media.
// Owner: TUI composer attachment orchestration.
#[tokio::test]
async fn completion_targets_pending_id_and_pending_attachments_cannot_submit() {
    let first_id = MediaAttachId::new();
    let second_id = MediaAttachId::new();
    let first_task = tokio::spawn(std::future::pending::<super::PastedMediaOutcome>());
    let second_task = tokio::spawn(async {
        super::PastedMediaOutcome::Image(ImageContent {
            data: "Y29tcGxldGVk".into(),
            mime_type: "image/webp".into(),
        })
    });
    let mut app = test_app();
    app.media_attach_tasks.extend([
        super::MediaAttachTask {
            id: first_id,
            task: first_task,
        },
        super::MediaAttachTask {
            id: second_id,
            task: second_task,
        },
    ]);
    app.input_ui
        .push_pending_attachment(first_id, "first.txt".into());
    app.input_ui
        .push_ready_attachment(ChatMedia::Image(ImageContent {
            data: "cmVhZHk=".into(),
            mime_type: "image/png".into(),
        }));
    app.input_ui
        .push_pending_attachment(second_id, "second.txt".into());

    let completion = super::next_media_attach_completion(&mut app.media_attach_tasks).await;
    assert_eq!(completion.id, second_id);
    app.finish_pasted_media(completion);

    let expected = vec![
        ComposerAttachment::Pending {
            id: first_id,
            name: "first.txt".into(),
        },
        ComposerAttachment::Ready(ChatMedia::Image(ImageContent {
            data: "cmVhZHk=".into(),
            mime_type: "image/png".into(),
        })),
        ComposerAttachment::Ready(ChatMedia::Image(ImageContent {
            data: "Y29tcGxldGVk".into(),
            mime_type: "image/webp".into(),
        })),
    ];
    assert_eq!(app.input_ui.attachments(), expected);
    assert!(app.input_ui.take_ready_media().is_err());
    assert_eq!(app.input_ui.attachments(), expected);
    assert_eq!(app.media_attach_tasks.len(), 1);
    assert_eq!(app.media_attach_tasks[0].id, first_id);

    app.cancel_all_pending_attachments();
}
