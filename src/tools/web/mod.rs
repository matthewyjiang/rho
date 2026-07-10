mod adapters;
mod display;
mod fetch;
mod process;
mod search;
mod storage;
mod util;

pub use adapters::{FetchContent, GetSearchContent, WebSearch};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
