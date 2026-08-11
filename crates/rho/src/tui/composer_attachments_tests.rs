use image::{DynamicImage, ImageFormat};
use ratatui_image::picker::{Picker, ProtocolType};
use rho_providers::model::ImageContent;
use std::io::Cursor;

use super::{layout_composer_attachments, ComposerAttachmentSlot, COMPOSER_IMAGE_GAP};
use crate::tui::{
    feed_image::{FeedImage, ImageRowBudget, COMPOSER_IMAGE_HEIGHT},
    ChatMedia, ChatTextDocument, MediaAttachId, PendingAttachmentSource,
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
