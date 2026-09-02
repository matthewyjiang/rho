use std::io;

use crossterm::event::{MouseButton, MouseEventKind};
use pretty_assertions::assert_eq;
use ratatui::{backend::TestBackend, Terminal};

use crate::tui::media_attach;

use super::{
    super::{tests::test_app, ComposerAttachment},
    ChatMedia, ChatTextDocument, Clipboard, CopyOutcome, ImageContent, MediaAttachId,
    PendingAttachmentSource,
};

struct FakeClipboard {
    text: String,
    paste_error: Option<String>,
}

impl Clipboard for FakeClipboard {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome> {
        self.text = text.to_string();
        Ok(CopyOutcome::Confirmed)
    }

    fn paste(&mut self) -> io::Result<String> {
        match self.paste_error.as_ref() {
            Some(message) => Err(io::Error::other(message.clone())),
            None => Ok(self.text.clone()),
        }
    }
}

async fn insert_external_paste_and_finish(app: &mut super::App, text: &str) {
    app.insert_external_paste(text);
    if !app.media_attach_tasks.is_empty() {
        let outcome = media_attach::next_media_attach_completion(&mut app.media_attach_tasks).await;
        app.finish_media_attach(outcome);
    }
}

// Covers: right-click inserts host clipboard text once (release does not paste again).
// Owner: tui clipboard
#[test]
fn right_click_pastes_clipboard_text_into_composer() {
    let mut app = test_app();
    app.clipboard = Box::new(FakeClipboard {
        text: "hello from clip".into(),
        paste_error: None,
    });
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Right),
        10,
        10,
        &mut terminal,
    )
    .unwrap();
    assert_eq!(app.input_ui.text(), "hello from clip");

    app.handle_mouse_event(
        MouseEventKind::Up(MouseButton::Right),
        10,
        10,
        &mut terminal,
    )
    .unwrap();
    assert_eq!(app.input_ui.text(), "hello from clip");
}

// Covers: empty clipboard is a no-op; a paste backend failure surfaces as a toast.
// Owner: tui clipboard
#[test]
fn clipboard_text_paste_empty_and_error() {
    let mut app = test_app();
    app.clipboard = Box::new(FakeClipboard {
        text: String::new(),
        paste_error: None,
    });
    app.paste_clipboard_text();
    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.status(), "");

    app.clipboard = Box::new(FakeClipboard {
        text: String::new(),
        paste_error: Some("no display".into()),
    });
    app.paste_clipboard_text();
    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.status(), "could not paste clipboard: no display");
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
        media_attach::MediaAttachOutcome::Unsupported {
            original_text: "archive.bin".into(),
        }
    });
    let id = MediaAttachId::new();
    let mut pending = vec![media_attach::MediaAttachTask { id, task }];

    let mut first_poll = Box::pin(media_attach::next_media_attach_completion(&mut pending));
    assert!(futures_util::poll!(&mut first_poll).is_pending());
    drop(first_poll);
    assert_eq!(pending.len(), 1);

    let _ = release_tx.send(());
    let completion = media_attach::next_media_attach_completion(&mut pending).await;
    assert_eq!(completion.id, id);
    assert!(matches!(
        completion.outcome,
        media_attach::MediaAttachOutcome::Unsupported { original_text }
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
            source: PendingAttachmentSource::File,
            name: "notes.txt".into(),
        }]
    );

    let outcome = media_attach::next_media_attach_completion(&mut app.media_attach_tasks).await;
    app.finish_media_attach(outcome);

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

// Covers: an unsupported attachment restored as text confirms the collapse when
// it is large enough to hide behind a marker (same rule as external pastes).
// Owner: tui status surface
#[tokio::test]
async fn unsupported_attach_restore_of_large_text_confirms_collapse() {
    let min_lines = crate::tui::paste_burst::PASTE_COLLAPSE_MIN_LINES;
    let original = (0..min_lines)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let task = tokio::spawn(async move {
        media_attach::MediaAttachOutcome::Unsupported {
            original_text: original,
        }
    });
    let id = MediaAttachId::new();
    let mut app = test_app();
    app.media_attach_tasks
        .push(media_attach::MediaAttachTask { id, task });
    app.input_ui
        .push_pending_attachment(id, PendingAttachmentSource::File, "archive.bin".into());

    let outcome = media_attach::next_media_attach_completion(&mut app.media_attach_tasks).await;
    app.finish_media_attach(outcome);

    assert!(app.input_ui.attachments().is_empty());
    assert_eq!(
        app.input_ui.text(),
        format!("[ pasted: {min_lines} lines ]")
    );
    assert_eq!(app.status(), format!("pasted {min_lines} lines"));
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
        media_attach::MediaAttachOutcome::Unsupported {
            original_text: "notes.txt".into(),
        }
    });
    let id = MediaAttachId::new();
    let mut app = test_app();
    app.media_attach_tasks
        .push(media_attach::MediaAttachTask { id, task });
    app.input_ui
        .push_pending_attachment(id, PendingAttachmentSource::File, "notes.txt".into());
    app.attach_ready_image(ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    });

    app.backspace_input();

    assert_eq!(
        app.input_ui.attachments(),
        &[ComposerAttachment::Pending {
            id,
            source: PendingAttachmentSource::File,
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
    let first_task = tokio::spawn(std::future::pending::<media_attach::MediaAttachOutcome>());
    let second_task = tokio::spawn(async {
        media_attach::MediaAttachOutcome::ready(ChatMedia::Image(ImageContent {
            data: "Y29tcGxldGVk".into(),
            mime_type: "image/webp".into(),
        }))
    });
    let mut app = test_app();
    app.media_attach_tasks.extend([
        media_attach::MediaAttachTask {
            id: first_id,
            task: first_task,
        },
        media_attach::MediaAttachTask {
            id: second_id,
            task: second_task,
        },
    ]);
    app.input_ui.push_pending_attachment(
        first_id,
        PendingAttachmentSource::File,
        "first.txt".into(),
    );
    app.input_ui.push_ready_attachment(
        ChatMedia::Image(ImageContent {
            data: "cmVhZHk=".into(),
            mime_type: "image/png".into(),
        }),
        None,
    );
    app.input_ui.push_pending_attachment(
        second_id,
        PendingAttachmentSource::File,
        "second.txt".into(),
    );

    let completion = media_attach::next_media_attach_completion(&mut app.media_attach_tasks).await;
    assert_eq!(completion.id, second_id);
    app.finish_media_attach(completion);

    let expected = vec![
        ComposerAttachment::Pending {
            id: first_id,
            source: PendingAttachmentSource::File,
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
