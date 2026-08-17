use pretty_assertions::assert_eq;
use serde_json::json;

use super::image_from_generation_call;

// Covers: wrapped or URL-safe payloads still type, store STANDARD data,
// and unknown bytes stay untyped.
// Owner: providers stream parse
#[test]
fn image_generation_normalizes_and_sniffs_base64() {
    let cases = [
        ("/9j/4AAQ", Some(("image/jpeg", "/9j/4AAQ"))),
        ("_9j_4AAQ", Some(("image/jpeg", "/9j/4AAQ"))),
        ("iVBORw0K\nGgo=", Some(("image/png", "iVBORw0KGgo="))),
        ("R0lGODlhAQAB", Some(("image/gif", "R0lGODlhAQAB"))),
        ("UklGRnh4eHhXRUJQ", Some(("image/webp", "UklGRnh4eHhXRUJQ"))),
        ("not-image", None),
        ("", None),
    ];
    for (data, expected) in cases {
        let image = image_from_generation_call(&json!({ "result": data }));
        match expected {
            Some((mime, stored)) => {
                let image = image.expect(data);
                assert_eq!(image.mime_type, mime, "{data:?}");
                assert_eq!(image.data, stored, "{data:?}");
            }
            None => assert_eq!(image, None, "{data:?}"),
        }
    }
}
