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
mod storage;
mod util;

pub use adapters::{GetSearchContent, WebSearch};
pub(super) use sdk_fetch_content::SdkFetchContent;
pub(super) use sdk_web_search::SdkWebSearch;

pub(crate) fn access_tools(config: &crate::config::Config) -> WebSearch {
    WebSearch::with_client(config, util::http_client())
}

pub(super) fn sdk_bundle(
    config: &crate::config::Config,
    capabilities: &crate::agent::AgentCapabilities,
    process_environment: rho_sdk::ProcessEnvironment,
) -> super::sdk_registry::StaticToolBundle {
    use crate::agent::ToolCapability;

    let mut tools = Vec::<Arc<dyn SdkTool>>::new();
    if capabilities.contains(&ToolCapability::WebSearch) {
        tools.push(Arc::new(SdkWebSearch::new(
            access_tools(config),
            config.max_output_bytes,
        )));
    }
    if capabilities.contains(&ToolCapability::FetchContent) {
        tools.push(Arc::new(SdkFetchContent::new(
            config.max_output_bytes,
            process_environment,
        )));
    }
    if capabilities.contains(&ToolCapability::GetSearchContent) {
        tools.push(Arc::new(sdk_get_search_content::SdkGetSearchContent::new(
            config.max_output_bytes,
        )));
    }
    super::sdk_registry::StaticToolBundle::new(tools)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
