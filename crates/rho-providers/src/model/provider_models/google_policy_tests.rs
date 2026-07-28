use super::*;

#[test]
fn text_chat_filter_hides_specialty_models() {
    assert!(is_text_chat_model("gemini-3.1-flash-lite"));
    assert!(is_text_chat_model("gemma-4-31b-it"));
    assert!(!is_text_chat_model("gemini-3.1-flash-image"));
    assert!(!is_text_chat_model("gemini-2.5-flash-preview-tts"));
    assert!(!is_text_chat_model("lyria-3-clip-preview"));
    assert!(!is_text_chat_model("nano-banana-pro-preview"));
}

