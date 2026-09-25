use image::{DynamicImage, ImageFormat};
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};
use rho_providers::model::ImageContent;
use std::io::Cursor;

use super::{
    attachment_target_at, layout_composer_attachments, AttachmentTarget, ComposerAttachmentSlot,
    COMPOSER_IMAGE_GAP,
};
use crate::tui::{
    feed_image::{FeedImage, ImageRowBudget, COMPOSER_IMAGE_HEIGHT},
    ChatMedia, ChatTextDocument, ComposerAttachment, MediaAttachId, PendingAttachmentSource,
};

fn kitty_picker() -> Picker {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    picker
}

fn png_asset(width: u32, height: u32) -> FeedImage {
    let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([10, 20, 30, 255]),
    ));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, ImageFormat::Png)
        .expect("encode png");
    FeedImage::decode(&encoded.into_inner())
        .expect("decode png")
        .to_feed_image(&kitty_picker())
}

fn image_slot(preview: FeedImage) -> ComposerAttachmentSlot {
    ComposerAttachmentSlot::ready(
        ChatMedia::Image(ImageContent {
            data: String::new(),
            mime_type: "image/png".into(),
        }),
        Some(preview),
    )
}

// Covers: consecutive image previews pack left-to-right at natural width with
// labels beneath; non-preview attachments stay full-width rows.
// Owner: pure layout policy
#[test]
fn composer_attachments_stack_image_previews_sideways_with_labels() {
    let tall = png_asset(300, 600);
    let wide = png_asset(600, 100);
    let slots = vec![
        image_slot(tall),
        image_slot(wide),
        ComposerAttachmentSlot::pending(
            MediaAttachId::new(),
            PendingAttachmentSource::File,
            "doc.pdf".into(),
        ),
        ComposerAttachmentSlot::ready(
            ChatMedia::TextDocument(ChatTextDocument {
                name: "notes.txt".into(),
                mime: "text/plain".into(),
                body: "hi".into(),
                truncated: false,
                warnings: Vec::new(),
            }),
            None,
        ),
    ];
    let layout = layout_composer_attachments(&slots, 80, ImageRowBudget::composer());

    assert_eq!(layout.images.len(), 2);
    // Tall 300x600 fixture saturates the composer height budget.
    assert_eq!(layout.images[0].height, usize::from(COMPOSER_IMAGE_HEIGHT));
    let packed_end = usize::from(layout.images[0].width)
        + COMPOSER_IMAGE_GAP
        + usize::from(layout.images[1].width);
    assert!(
        packed_end <= 80,
        "strip should fit within the composer width ({packed_end} <= 80)"
    );
    // strip + label + pending + doc
    assert_eq!(layout.total_rows, layout.images[0].height + 1 + 1 + 1);
    assert_eq!(layout.lines.len(), layout.total_rows);
    assert_eq!(layout.images[0].column, 0);
    assert_eq!(
        usize::from(layout.images[1].column),
        usize::from(layout.images[0].width) + COMPOSER_IMAGE_GAP
    );
    assert_eq!(layout.images[0].row, layout.images[1].row);
    assert_eq!(layout.images[0].height, layout.images[1].height);
    // First rows are blank image cells; last three rows are labels.
    assert!(layout.lines[0].spans.is_empty() || layout.lines[0].to_string().trim().is_empty());
}

// Covers: when one strip cannot hold every image + gaps, the run wraps so each
// preview stays within the composer width.
// Owner: pure layout policy
#[test]
fn composer_image_run_wraps_when_gaps_exceed_width() {
    // gap=2: max_per_strip at width 10 is (10+2)/(1+2)=4. Five images need two strips.
    let width = 10usize;
    let slots: Vec<_> = (0..5).map(|_| image_slot(png_asset(40, 40))).collect();
    let layout = layout_composer_attachments(&slots, width, ImageRowBudget::composer());

    assert_eq!(layout.images.len(), 5);
    let rows: Vec<usize> = layout.images.iter().map(|image| image.row).collect();
    let distinct_rows: std::collections::BTreeSet<usize> = rows.iter().copied().collect();
    assert!(
        distinct_rows.len() >= 2,
        "expected multiple strips, got rows {rows:?}"
    );
    assert_eq!(
        layout
            .images
            .iter()
            .filter(|image| image.row == layout.images[0].row)
            .count(),
        4,
        "first strip should hold four images at width {width}"
    );
    assert_eq!(
        layout
            .images
            .iter()
            .filter(|image| image.row != layout.images[0].row)
            .count(),
        1,
        "remainder should wrap to a second strip"
    );
    for image in &layout.images {
        let end = usize::from(image.column) + usize::from(image.width);
        assert!(
            end <= width,
            "placement overflows width: column={} width={} end={end} > {width}",
            image.column,
            image.width
        );
    }
    assert_eq!(layout.total_rows, layout.lines.len());
}

// Covers: only the painted `✕` button removes an attachment. A press on the
// image, on the label text, between previews, on a row scrolled out of the
// composer window, or outside the composer is not a target, so a stray click
// on a preview can never delete it.
// Owner: pure layout policy (attachment pointer targets).
#[test]
fn only_the_remove_button_is_an_attachment_target() {
    let slots = vec![
        image_slot(png_asset(40, 40)),
        image_slot(png_asset(40, 40)),
        ComposerAttachmentSlot::pending(
            MediaAttachId::new(),
            PendingAttachmentSource::File,
            "doc.pdf".into(),
        ),
    ];
    let layout = layout_composer_attachments(&slots, 40, ImageRowBudget::composer());
    let button = |attachment: usize| {
        layout
            .targets
            .iter()
            .find(|target| target.attachment == attachment)
            .expect("every attachment paints a remove button")
            .clone()
    };
    let (first, second, doc) = (button(0), button(1), button(2));
    let area = Rect::new(5, 10, 40, layout.total_rows as u16);
    let at = |target: &AttachmentTarget, start: usize| {
        (5 + target.columns.start, 10 + (target.row - start) as u16)
    };
    // (name, first visible line, pointer cell, expected attachment)
    let cases = [
        ("first image button", 0, at(&first, 0), Some(0)),
        ("second image button", 0, at(&second, 0), Some(1)),
        ("document button", 0, at(&doc, 0), Some(2)),
        ("image cell above its label", 0, (5, 10), None),
        (
            "label text left of the button",
            0,
            (5 + first.columns.start - 2, 10 + first.row as u16),
            None,
        ),
        (
            "button row scrolled out of the window",
            first.row + 1,
            (5 + first.columns.start, 10),
            None,
        ),
        ("outside the composer", 0, (4, 10 + first.row as u16), None),
    ];
    for (name, start, (column, row), expected) in cases {
        assert_eq!(
            attachment_target_at(&layout, area, start, column, row).map(|target| target.attachment),
            expected,
            "{name}"
        );
    }
}

// Covers: removing a middle attachment by index (the `✕` path) drops exactly
// that slot, reports the pending count when it was still extracting, and
// leaves the other pending extraction in place.
// Owner: attachment removal policy (shared by Backspace and the pointer).
#[test]
fn removing_a_middle_attachment_keeps_its_neighbours() {
    let document = |name: &str| {
        ChatMedia::TextDocument(ChatTextDocument {
            name: name.into(),
            mime: "text/plain".into(),
            body: "hi".into(),
            truncated: false,
            warnings: Vec::new(),
        })
    };
    let (first_pending, second_pending) = (MediaAttachId::new(), MediaAttachId::new());
    let mut app = crate::tui::tests::test_app();
    app.input_ui.push_ready_attachment(document("a.txt"), None);
    app.input_ui.push_pending_attachment(
        first_pending,
        PendingAttachmentSource::File,
        "b.pdf".into(),
    );
    app.input_ui.push_pending_attachment(
        second_pending,
        PendingAttachmentSource::File,
        "c.pdf".into(),
    );

    app.remove_composer_attachment(1);

    assert_eq!(
        (app.input_ui.attachments(), app.status().to_string()),
        (
            vec![
                ComposerAttachment::Ready(document("a.txt")),
                ComposerAttachment::Pending {
                    id: second_pending,
                    source: PendingAttachmentSource::File,
                    name: "c.pdf".into(),
                },
            ],
            "extracting files: 1".to_string(),
        )
    );
}

// Covers: removal reflows the label row so the next attachment's `✕` can
// land under the same cell; a double click there removes only one
// attachment, while a later separate click removes the next.
// Owner: attachment pointer removal (the one destructive chrome click).
#[test]
fn double_click_on_remove_button_removes_one_attachment() {
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::{backend::TestBackend, Terminal};

    let document = |name: &str| {
        ChatMedia::TextDocument(ChatTextDocument {
            name: name.into(),
            mime: "text/plain".into(),
            body: "hi".into(),
            truncated: false,
            warnings: Vec::new(),
        })
    };
    let mut app = crate::tui::tests::test_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    // Same-width names so every button sits in the same columns.
    for name in ["a.txt", "b.txt", "c.txt"] {
        app.input_ui.push_ready_attachment(document(name), None);
    }
    // Screen cells of every painted remove button, in attachment order.
    let buttons = |app: &mut crate::tui::App| {
        let ctx = app.frame_context(Rect::new(0, 0, 80, 24));
        app.composer_attachment_layout(80)
            .targets
            .iter()
            .map(|target| {
                (
                    ctx.layout.composer.x + target.columns.start,
                    ctx.layout.composer.y + (target.row - ctx.layout.composer_start) as u16,
                )
            })
            .collect::<Vec<_>>()
    };
    let press = |app: &mut crate::tui::App, terminal: &mut Terminal<TestBackend>, cell| {
        let (column, row) = cell;
        app.handle_mouse_event(
            MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            terminal,
        )
        .unwrap();
        app.handle_mouse_event(MouseEventKind::Up(MouseButton::Left), column, row, terminal)
            .unwrap();
    };

    terminal.draw(|frame| app.draw(frame)).unwrap();
    // The composer is bottom-anchored and shrinks a row per removal, so the
    // label above the removed one slides down into the pointer's cell.
    let cell = *buttons(&mut app).last().unwrap();
    press(&mut app, &mut terminal, cell);
    terminal.draw(|frame| app.draw(frame)).unwrap();
    assert_eq!(
        buttons(&mut app).last(),
        Some(&cell),
        "reflow puts b.txt's button under the pointer"
    );
    press(&mut app, &mut terminal, cell);
    assert_eq!(
        app.input_ui.attachments(),
        vec![
            ComposerAttachment::Ready(document("a.txt")),
            ComposerAttachment::Ready(document("b.txt")),
        ],
        "the second press of a double click is swallowed"
    );

    app.input_ui.cancel_pointer_click_sequence();
    press(&mut app, &mut terminal, cell);
    assert_eq!(
        app.input_ui.attachments(),
        vec![ComposerAttachment::Ready(document("a.txt"))],
        "a separate click still removes"
    );
}
