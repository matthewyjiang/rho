use pretty_assertions::assert_eq;
use rho_providers::credentials::MemoryCredentialStore;

use super::*;

fn picker_values(picker: &UiPicker) -> Vec<String> {
    picker.items.iter().map(|item| item.value.clone()).collect()
}

fn config_with(
    mode: WebSearchMode,
    backend: SearchBackend,
    openai: OpenAiSearchConnection,
) -> Config {
    let mut config = Config::default();
    config.web_search.mode = mode;
    config.web_search.backend = backend;
    config.web_search.openai.connection = openai;
    config
}

// Covers: mode and backend changes must not add or remove web search rows
// Owner: tui web search config
#[test]
fn main_picker_rows_stay_stable_across_mode_and_backend() {
    let baseline = picker_values(&main_picker(
        &config_with(
            WebSearchMode::Auto,
            SearchBackend::OpenAi,
            OpenAiSearchConnection::Api,
        ),
        "openai",
        "gpt-5",
    ));
    for mode in WebSearchMode::ALL {
        for backend in SearchBackend::ALL {
            assert_eq!(
                picker_values(&main_picker(
                    &config_with(mode, backend, OpenAiSearchConnection::Api),
                    "openai",
                    "gpt-5",
                )),
                baseline,
            );
        }
    }
}

fn openai_rows(connection: OpenAiSearchConnection) -> Vec<String> {
    let store = MemoryCredentialStore::default();
    let config = config_with(WebSearchMode::Backend, SearchBackend::OpenAi, connection);
    picker_values(&backend_picker(SearchBackend::OpenAi, &config, &store))
}

// Covers: OpenAI page always shows Codex and API settings, regardless of connection
// Owner: tui web search config
#[test]
fn openai_page_shows_codex_and_api_settings() {
    let expected = vec![
        WEB_SEARCH_OPENAI_CONNECTION_VALUE.to_string(),
        WEB_SEARCH_CODEX_ENDPOINT_VALUE.to_string(),
        WebSearchUrlField::OpenAiApiBase.value().to_string(),
        WebSearchUrlField::OpenAiApiBase.reset_value().to_string(),
        WEB_SEARCH_OPENAI_KEY_VALUE.to_string(),
    ];
    assert_eq!(openai_rows(OpenAiSearchConnection::Api), expected);
    assert_eq!(openai_rows(OpenAiSearchConnection::Codex), expected);
}
