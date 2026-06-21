use crate::model::{AuthMode, ModelError, OpenAiProvider};

pub fn reasoning_config_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn build_provider(
    provider: &str,
    model: &str,
    _auth: &str,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
) -> anyhow::Result<OpenAiProvider> {
    match provider {
        "openai" => OpenAiProvider::new_with_reasoning(
            model.to_string(),
            AuthMode::ApiKey,
            reasoning_effort,
            reasoning_summary,
        )
        .map_err(Into::into),
        "openai-codex" => OpenAiProvider::new_with_reasoning(
            model.to_string(),
            AuthMode::Codex,
            reasoning_effort,
            reasoning_summary,
        )
        .map_err(Into::into),
        other => Err(ModelError::UnsupportedProvider(other.to_string()).into()),
    }
}
