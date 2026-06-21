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
    auth: &str,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
) -> anyhow::Result<OpenAiProvider> {
    match provider {
        "openai" => {
            let auth_mode = match auth {
                "codex" => AuthMode::Codex,
                _ => AuthMode::ApiKey,
            };
            OpenAiProvider::new_with_reasoning(
                model.to_string(),
                auth_mode,
                reasoning_effort,
                reasoning_summary,
            )
            .map_err(Into::into)
        }
        other => Err(ModelError::UnsupportedProvider(other.to_string()).into()),
    }
}
