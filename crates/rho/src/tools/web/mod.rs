mod adapters;
mod fetch;
mod sdk_fetch_content;
mod search;
mod storage;
mod util;

pub use adapters::{GetSearchContent, WebSearch};
pub(super) use sdk_fetch_content::SdkFetchContent;

pub(crate) fn access_tools(config: &crate::config::Config) -> WebSearch {
    WebSearch::with_client(config, util::http_client())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
