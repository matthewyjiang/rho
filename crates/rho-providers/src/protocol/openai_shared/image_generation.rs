use serde_json::json;

use crate::model::{ContentBlock, ImageContent, ProviderContextBlock};

use super::compact::COMPACTION_OUTPUT_ITEM_KIND;

const IMAGE_GENERATION_CALL: &str = "image_generation_call";

pub(super) fn is_image_generation_call(item: &serde_json::Value) -> bool {
    item.get("type").and_then(|value| value.as_str()) == Some(IMAGE_GENERATION_CALL)
}

pub(super) fn is_image_generation_replay(block: &ProviderContextBlock) -> bool {
    block.kind == COMPACTION_OUTPUT_ITEM_KIND
        && block.data.get("type").and_then(serde_json::Value::as_str) == Some(IMAGE_GENERATION_CALL)
}

pub(super) fn slim_image_generation_item(item: &serde_json::Value) -> serde_json::Value {
    let mut data = item.clone();
    if let Some(obj) = data.as_object_mut() {
        obj.remove("result");
    }
    data
}

pub(super) fn image_from_generation_call(item: &serde_json::Value) -> Option<ImageContent> {
    let data = item
        .get("result")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mime_type = image_mime_from_base64(data)?.to_owned();
    Some(ImageContent {
        mime_type,
        data: data.to_owned(),
    })
}

/// Rebuilds `result` on slim replay items from assistant Image blocks, in order.
///
/// Live persist strips the base64 payload so the session does not store it
/// twice. Older sessions that already kept `result` are left alone.
pub(super) fn restore_image_generation_results(
    replay: &mut [ProviderContextBlock],
    content: &[ContentBlock],
) {
    let mut images = content.iter().filter_map(|block| match block {
        ContentBlock::Image(image) => Some(image.data.as_str()),
        ContentBlock::Text(_) | ContentBlock::ToolCall(_) => None,
    });
    for block in replay {
        if !is_image_generation_replay(block) {
            continue;
        }
        let Some(obj) = block.data.as_object_mut() else {
            continue;
        };
        let has_result = obj
            .get("result")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if has_result {
            continue;
        }
        if let Some(data) = images.next() {
            obj.insert("result".into(), json!(data));
        }
    }
}

fn image_mime_from_base64(data: &str) -> Option<&'static str> {
    // 16 base64 chars decode to 12 bytes, enough for PNG/JPEG/GIF/WebP.
    const PREFIX_CHARS: usize = 16;
    let mut prefix = String::with_capacity(PREFIX_CHARS);
    for byte in data.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        prefix.push(char::from(byte));
        if prefix.len() == PREFIX_CHARS {
            break;
        }
    }
    ImageContent::mime_type_from_bytes(&decode_base64_prefix(&prefix)?)
}

fn decode_base64_prefix(data: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    if data.is_empty() {
        return None;
    }
    let mut padded = data.to_string();
    while !padded.len().is_multiple_of(4) {
        padded.push('=');
    }
    base64::engine::general_purpose::STANDARD
        .decode(&padded)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&padded))
        .ok()
}

#[cfg(test)]
#[path = "image_generation_tests.rs"]
mod tests;
