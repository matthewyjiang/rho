use pretty_assertions::assert_eq;

use super::*;
use crate::model::{ContentBlock, ImageContent, ModelEvent, ModelResponse};

const JPEG_BASE64: &str = "/9j/4AAQ";
const PNG_BASE64: &str = "iVBORw0KGgo=";

// Covers: hosted image_generation_call must surface activity, persist a slim
// replay item, and keep the image so an image-only turn is valid content.
// Owner: providers stream parse
#[test]
fn image_generation_call_emits_activity_and_slim_replay() {
    let mut state = CodexSseState::default();
    let mut activities = Vec::new();
    let mut replay_items = Vec::new();
    handle_codex_sse_line(
        &format!(
            r#"data: {{"type":"response.output_item.done","output_index":0,"item":{{"type":"image_generation_call","id":"ig_1","status":"completed","prompt":"a corgi surfing","result":"{JPEG_BASE64}"}}}}"#
        ),
        &mut state,
        &mut Some(&mut |event| {
            if let Some((name, detail)) = event.as_hosted_tool_activity() {
                activities.push((name.to_owned(), detail.to_owned()));
            }
            if let ModelEvent::ProviderContext { kind, data, .. } = &event {
                if kind == "openai_response_output_item"
                    && data.get("type").and_then(|value| value.as_str())
                        == Some("image_generation_call")
                {
                    replay_items.push(data.clone());
                }
            }
            Ok(())
        }),
    )
    .unwrap();
    handle_codex_sse_line(
        &format!(
            r#"data: {{"type":"response.completed","response":{{"id":"resp_1","output":[{{"type":"image_generation_call","id":"ig_1","status":"completed","prompt":"a corgi surfing","result":"{JPEG_BASE64}"}}]}}}}"#
        ),
        &mut state,
        &mut Some(&mut |event| {
            if let Some((name, detail)) = event.as_hosted_tool_activity() {
                activities.push((name.to_owned(), detail.to_owned()));
            }
            Ok(())
        }),
    )
    .unwrap();

    assert_eq!(
        activities,
        vec![(
            "image_generation".to_string(),
            "a corgi surfing".to_string()
        )]
    );
    assert_eq!(replay_items.len(), 1);
    assert!(replay_items[0].get("result").is_none());
    assert_eq!(
        replay_items[0]
            .get("prompt")
            .and_then(|value| value.as_str()),
        Some("a corgi surfing")
    );
    let CodexSseResponse { response, .. } = state.into_response().unwrap();
    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Image(ImageContent {
            data: JPEG_BASE64.into(),
            mime_type: "image/jpeg".into(),
        })])
    );
}

// Covers: completed-only image_generation_call items still become assistant images.
// Owner: providers stream parse
#[test]
fn completed_only_image_generation_is_content() {
    let mut state = CodexSseState::default();
    handle_codex_sse_line(
        &format!(
            r#"data: {{"type":"response.completed","response":{{"id":"resp_1","output":[{{"type":"image_generation_call","id":"ig_1","status":"completed","prompt":"night lighthouse","result":"{PNG_BASE64}"}}]}}}}"#
        ),
        &mut state,
        &mut None,
    )
    .unwrap();

    let CodexSseResponse { response, .. } = state.into_response().unwrap();
    assert_eq!(
        response,
        ModelResponse::Assistant(vec![ContentBlock::Image(ImageContent {
            data: PNG_BASE64.into(),
            mime_type: "image/png".into(),
        })])
    );
}

// Covers: unknown or invalid image payloads must not be stored as fake JPEGs.
// Owner: providers stream parse
#[test]
fn invalid_image_generation_result_is_ignored() {
    let mut state = CodexSseState::default();
    handle_codex_sse_line(
        r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"image_generation_call","id":"ig_1","status":"completed","prompt":"nope","result":"not-image"}]}}"#,
        &mut state,
        &mut None,
    )
    .unwrap();

    assert!(state.into_response().is_err());
}
