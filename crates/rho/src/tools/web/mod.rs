use std::sync::Arc;

use rho_sdk::tool::Tool as SdkTool;

mod adapters;
mod fetch;
mod fetch_response;
mod output;
mod sdk_fetch_content;
pub(super) mod sdk_get_search_content;
pub(super) mod sdk_web_search;
mod search;
mod ssrf;
pub(crate) mod storage;
mod util;

pub use adapters::{GetSearchContent, WebSearch};
pub(super) use sdk_fetch_content::SdkFetchContent;
pub(super) use sdk_web_search::SdkWebSearch;
pub use storage::WebAccessStore;

/// Whether the active chat provider can run hosted `web_search` for this model.
pub(crate) fn supports_hosted_web_search(provider: &str, model: &str) -> bool {
    match provider {
        "openai" => true,
        "openai-codex" => !matches!(model, "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"),
        "xai" => true,
        _ => false,
    }
}

/// Native chat-provider search is the selected route for this config.
pub(crate) fn hosted_web_search_active(config: &crate::config::Config) -> bool {
    matches!(
        crate::config::web_search_route(
            &config.web_search,
            supports_hosted_web_search(&config.provider, &config.model),
        ),
        crate::config::WebSearchRoute::Native
    )
}

/// Client `web_search` tool for this config, if native search or a ready backend
/// should expose it.
pub(super) fn sdk_web_search(
    config: &crate::config::Config,
    store: WebAccessStore,
    max_output_bytes: usize,
) -> Option<SdkWebSearch> {
    let tool = access_tools_with_store(config, store);
    (hosted_web_search_active(config) || tool.client_available())
        .then(|| SdkWebSearch::new(tool, max_output_bytes))
}

pub(crate) const WEB_SEARCH_TOOL_NAME: &str = "web_search";

/// Hits the selected backend with a fixed query, independent of mode.
pub(crate) async fn test_search_backend(
    config: &crate::config::Config,
) -> Result<usize, rho_tools::tool::ToolError> {
    let items = search::run_search_query(
        &util::http_client(),
        "rho web search connection test",
        1,
        None,
        None,
        &search::SearchBackendConfig::from_config(config),
    )
    .await?;
    Ok(items.len())
}

#[cfg(test)]
pub(crate) fn access_tools(config: &crate::config::Config) -> WebSearch {
    access_tools_with_store(config, WebAccessStore::new())
}

pub(crate) fn access_tools_with_store(
    config: &crate::config::Config,
    store: WebAccessStore,
) -> WebSearch {
    WebSearch::with_client(config, util::http_client(), store)
}

pub(super) fn sdk_bundle(
    config: &crate::config::Config,
    capabilities: &crate::agent::AgentCapabilities,
    process_environment: rho_sdk::ProcessEnvironment,
    store: WebAccessStore,
) -> super::sdk_registry::StaticToolBundle {
    use crate::agent::ToolCapability;

    let mut tools = Vec::<Arc<dyn SdkTool>>::new();
    if capabilities.contains(&ToolCapability::WebSearch) {
        if let Some(tool) = sdk_web_search(config, store.clone(), config.max_output_bytes) {
            tools.push(Arc::new(tool));
        }
    }
    if capabilities.contains(&ToolCapability::FetchContent) {
        tools.push(Arc::new(SdkFetchContent::new(
            config.max_output_bytes,
            process_environment,
            store.clone(),
        )));
    }
    if capabilities.contains(&ToolCapability::GetSearchContent) {
        tools.push(Arc::new(sdk_get_search_content::SdkGetSearchContent::new(
            config.max_output_bytes,
            store,
        )));
    }
    super::sdk_registry::StaticToolBundle::new(tools)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
mod performance_benchmarks;
