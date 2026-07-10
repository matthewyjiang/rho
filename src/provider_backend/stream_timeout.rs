use std::{future::Future, time::Duration};

pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds the shared HTTP client for streamed model responses.
///
/// The read timeout resets whenever bytes arrive, so long-running responses
/// remain supported while a silent or stale connection eventually returns
/// control to the TUI.
pub(crate) fn streaming_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STREAM_IDLE_TIMEOUT)
        .build()
        .expect("streaming HTTP client configuration should be valid")
}

pub(crate) async fn wait_for_stream_activity<F>(future: F) -> Result<F::Output, super::ModelError>
where
    F: Future,
{
    wait_for_stream_activity_for(future, STREAM_IDLE_TIMEOUT).await
}

pub(crate) async fn wait_for_stream_activity_for<F>(
    future: F,
    idle_timeout: Duration,
) -> Result<F::Output, super::ModelError>
where
    F: Future,
{
    tokio::time::timeout(idle_timeout, future)
        .await
        .map_err(|_| super::ModelError::StreamIdleTimeout {
            timeout: idle_timeout,
        })
}

#[cfg(test)]
#[path = "stream_timeout_tests.rs"]
mod tests;
