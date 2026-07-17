use std::time::Duration;

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    CancellationToken, ReasoningLevel,
};

use crate::providers::build_sdk_provider;

pub(super) async fn generate_session_title(
    provider_name: String,
    model: String,
    first_user_message: String,
) -> anyhow::Result<String> {
    let provider = build_sdk_provider(&provider_name, &model, ReasoningLevel::Low)?;
    let request_messages = vec![
        Message::System(
            "Generate a concise title for this chat session. Return only the title, no quotes, no punctuation at the end. Use 3 to 7 words."
                .into(),
        ),
        Message::user_text(format!("First user message:\n\n{first_user_message}")),
    ];
    let cancellation = CancellationToken::new();
    let request = crate::usage::send_recorded(
        provider.as_ref(),
        ModelRequest {
            messages: &request_messages,
            tools: &[],
            cancellation: cancellation.clone(),
            reasoning_level: ReasoningLevel::Low,
            prompt_cache_key: None,
        },
        "title",
        crate::usage::default_recorder(),
    );
    tokio::pin!(request);
    let (result, timed_out) = tokio::select! {
        result = &mut request => (result, false),
        () = tokio::time::sleep(Duration::from_secs(20)) => {
            cancellation.cancel();
            (request.await, true)
        }
    };
    let (response, _) = match result {
        Err(_) if timed_out => {
            return Err(anyhow::anyhow!("title generation timed out"));
        }
        result => result?,
    };
    let ModelResponse::Assistant(blocks) = response;
    let title = blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    sanitize_session_title(&title)
        .ok_or_else(|| anyhow::anyhow!("title model returned an empty title"))
}

pub(super) fn sanitize_session_title(title: &str) -> Option<String> {
    let title = title
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*' | '#'))
        .trim()
        .trim_end_matches(['.', ':', ';'])
        .trim();
    if title.is_empty() {
        return None;
    }
    let mut title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > 80 {
        title = title.chars().take(79).collect();
        title.push('…');
    }
    Some(title)
}
