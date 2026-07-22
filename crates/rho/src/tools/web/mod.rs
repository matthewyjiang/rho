use std::sync::Arc;

use rho_sdk::tool::Tool as SdkTool;

mod adapters;
mod fetch;
mod sdk_fetch_content;
pub(super) mod sdk_get_search_content;
mod search;
mod ssrf;
mod storage;
mod util;

pub use adapters::{GetSearchContent, WebSearch};
pub(super) use sdk_fetch_content::SdkFetchContent;

pub(crate) fn access_tools(config: &crate::config::Config) -> WebSearch {
    WebSearch::with_client(config, util::http_client())
}

pub(super) fn sdk_bundle(
    config: &crate::config::Config,
    capabilities: &crate::agent::AgentCapabilities,
) -> super::sdk_registry::StaticToolBundle {
    use crate::agent::ToolCapability;

    let mut tools = Vec::<Arc<dyn SdkTool>>::new();
    if capabilities.contains(&ToolCapability::WebSearch) {
        tools.push(
            rho_tools::legacy_sdk_adapter::web_search(
                access_tools(config),
                config.max_output_bytes,
            )
            .expect("web_search is a supported legacy tool"),
        );
    }
    if capabilities.contains(&ToolCapability::FetchContent) {
        tools.push(Arc::new(SdkFetchContent::new(config.max_output_bytes)));
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
