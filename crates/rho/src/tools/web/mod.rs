use std::sync::Arc;

use rho_sdk::tool::Tool as SdkTool;

mod adapters;
mod fetch;
mod fetch_response;
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
