mod adapters;
mod fetch;
mod process;
mod search;
mod storage;
mod util;

pub use adapters::{FetchContent, GetSearchContent, WebSearch};

pub(super) fn access_tools(config: &crate::config::Config) -> (WebSearch, FetchContent) {
    let client = util::http_client();
    (
        WebSearch::with_client(config, client.clone()),
        FetchContent::with_client(client),
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
