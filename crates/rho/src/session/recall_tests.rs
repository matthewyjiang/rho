use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::*;

// Covers: a saved original is recalled by id and paged by character window;
// unknown and malformed (path-escaping) ids fail clearly; oversized windows
// hit the output budget.
// Owner: recall storage.
#[test]
fn recalls_saved_original_by_id() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("recall");
    let result = ToolResult::failed("call_0", "héllo world");
    save(&dir, std::slice::from_ref(&result)).unwrap();
    let id = crate::compaction::recall_id(&result);
    let recall = |recall_id: &str, start: usize, chars: usize, budget: usize| {
        let request = RecallRequest {
            recall_id: recall_id.into(),
            start,
            chars,
        };
        super::recall(&dir, &request, budget)
            .map(|output| serde_json::from_str::<Value>(&output).unwrap())
            .map_err(|error| error.to_string())
    };

    assert_eq!(
        recall(&id, 1, 4, 4_096),
        Ok(json!({
            "note": UNTRUSTED,
            "recall_id": id,
            "tool_call_id": "call_0",
            "ok": false,
            "text": "éllo",
            "start": 1,
            "end": 5,
            "total_chars": 11,
            "next_start": 5,
        }))
    );
    for unknown in ["r0000000000000000", "../../etc/passwd"] {
        assert_eq!(
            recall(unknown, 0, 10, 4_096),
            Err(format!(
                "unknown recall_id '{unknown}': no elided tool result with this id in the current session"
            ))
        );
    }
    assert!(recall(&id, 0, 10, 16)
        .unwrap_err()
        .starts_with("sessions output byte budget: limit 16"));
}

// Covers: originals that differ only in images get their own recall ids, so
// saving the second never skips it as already stored.
// Owner: recall storage.
#[test]
fn image_only_differences_save_separate_originals() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("recall");
    let image = |data: &str| rho_sdk::model::ImageContent {
        mime_type: "image/png".into(),
        data: data.into(),
    };
    let first = ToolResult::succeeded("call_0", "shot").with_images(vec![image("a")]);
    let second = ToolResult::succeeded("call_0", "shot").with_images(vec![image("b")]);
    save(&dir, &[first.clone(), second.clone()]).unwrap();

    for original in [first, second] {
        let path = dir.join(format!("{}.json", crate::compaction::recall_id(&original)));
        let stored: ToolResult = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(stored, original);
    }
}
