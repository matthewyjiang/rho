use std::sync::Arc;

use rho_sdk::tool::Tool as SdkTool;

mod adapters;
mod fetch;
mod guard;
mod sdk_fetch_content;
mod search;
mod storage;
mod util;

pub use adapters::{GetSearchContent, WebSearch};
pub(super) use sdk_fetch_content::SdkFetchContent;

pub(crate) fn access_tools(config: &crate::config::Config) -> WebSearch {
    let access = guard::NetworkAccess::from_env();
    WebSearch::with_client(config, util::http_client(access), access)
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
        tools.push(Arc::new(SdkFetchContent::new(
            config.max_output_bytes,
            guard::NetworkAccess::from_env(),
        )));
    }
    if capabilities.contains(&ToolCapability::GetSearchContent) {
        tools.push(
            rho_tools::legacy_sdk_adapter::get_search_content(
                GetSearchContent,
                config.max_output_bytes,
            )
            .expect("get_search_content is a supported legacy tool"),
        );
    }
    super::sdk_registry::StaticToolBundle::new(tools)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
