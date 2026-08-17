use pretty_assertions::assert_eq;
use serde_json::json;

use super::{image_from_generation_call, image_mime_from_base64};

// Covers: mime sniff uses a 12-byte prefix so wrapped or URL-safe payloads
// still type, and unknown bytes stay untyped.
// Owner: providers stream parse
#[test]
fn image_generation_sniffs_mime_from_base64_prefix() {
    let cases = [
        ("/9j/4AAQ", Some("image/jpeg")),
        ("_9j_4AAQ", Some("image/jpeg")),
        ("iVBORw0K\nGgo=", Some("image/png")),
        ("R0lGODlhAQAB", Some("image/gif")),
        ("UklGRnh4eHhXRUJQ", Some("image/webp")),
        ("not-image", None),
        ("", None),
    ];
    for (data, expected) in cases {
        assert_eq!(image_mime_from_base64(data), expected, "{data:?}");
        assert_eq!(
            image_from_generation_call(&json!({ "result": data })).map(|image| image.mime_type),
            expected.map(str::to_owned),
            "{data:?}"
        );
    }
}
