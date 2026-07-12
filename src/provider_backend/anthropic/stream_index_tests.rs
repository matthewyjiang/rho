use super::*;

#[test]
fn rejects_out_of_range_content_block_index() {
    let mut state = AnthropicSseState::default();
    let err = handle_anthropic_stream_line(
        r#"data: {"type":"content_block_start","index":4000000000,"content_block":{"type":"text","text":""}}"#,
        &mut state,
        &mut |_| Ok(()),
    )
    .unwrap_err();

    assert!(matches!(
        err,
        ModelError::InvalidResponse(message)
            if message == "stream block index 4000000000 out of range"
    ));
    assert!(state.blocks.is_empty());
}
