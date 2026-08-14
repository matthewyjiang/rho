use super::*;

// Covers: thinking-only or blank chat completions must retry, not kill the run
// Owner: openai chat completions response conversion
#[test]
fn empty_chat_assistant_is_retryable() {
    let error = finalize_chat_assistant(
        String::new(),
        "thoughts only".into(),
        Vec::new(),
        ChatToolCallPolicy::Strict,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ModelError::RetryableInvalidResponse { error_type, .. } if error_type == "empty_assistant"
    ));
}
