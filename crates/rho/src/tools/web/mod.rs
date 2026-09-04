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

/// Hosted search is configured on and the active chat path can run it.
pub(crate) fn hosted_web_search_active(config: &crate::config::Config) -> bool {
    config.web_search_hosted && supports_hosted_web_search(&config.provider, &config.model)
}

/// Client backup backend can run for this config.
pub(crate) fn backup_web_search_available(config: &crate::config::Config) -> bool {
    access_tools(config).backup_available()
}

/// `web_search` capability is on when hosted search can run or a backup backend is ready.
pub(crate) fn web_search_available(config: &crate::config::Config) -> bool {
    hosted_web_search_active(config) || backup_web_search_available(config)
}

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
        tools.push(Arc::new(SdkWebSearch::new(
            access_tools_with_store(config, store.clone()),
            config.max_output_bytes,
        )));
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
