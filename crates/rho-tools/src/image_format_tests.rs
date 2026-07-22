use pretty_assertions::assert_eq;

use super::supported_image_mime_type;

#[test]
fn recognizes_supported_image_signatures() {
    assert_eq!(
        supported_image_mime_type(b"\x89PNG\r\n\x1a\nrest"),
        Some("image/png")
    );
    assert_eq!(
        supported_image_mime_type(b"\xff\xd8\xffrest"),
        Some("image/jpeg")
    );
    assert_eq!(supported_image_mime_type(b"GIF89arest"), Some("image/gif"));
    assert_eq!(
        supported_image_mime_type(b"RIFFxxxxWEBP"),
        Some("image/webp")
    );
    assert_eq!(supported_image_mime_type(b"plain text"), None);
}
