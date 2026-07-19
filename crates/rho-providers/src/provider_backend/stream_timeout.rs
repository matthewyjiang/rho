use std::{future::Future, time::Duration};

use tokio::time::Instant;

pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(test)]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds the shared HTTP client for provider requests.
///
/// Connection establishment is bounded, while stream idle timeouts are applied
/// when receiving and parsing streamed provider events. Keeping the client free
/// of a read timeout allows non-streaming requests to run as long as needed.
#[cfg(test)]
pub(crate) fn provider_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .expect("provider HTTP client configuration should be valid")
}

/// Tracks the time since a provider emitted a meaningful stream event.
///
/// Call [`Self::record_activity`] only after processing a provider payload.
/// Transport keep-alives such as SSE pings and WebSocket control frames must
/// not reset the deadline.
pub(crate) struct StreamIdleDeadline {
    timeout: Duration,
    last_activity: Instant,
}

impl StreamIdleDeadline {
    pub(crate) fn new() -> Self {
        Self::with_timeout(STREAM_IDLE_TIMEOUT)
    }

    pub(crate) fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            last_activity: Instant::now(),
        }
    }

    pub(crate) async fn wait_for<F>(&self, future: F) -> Result<F::Output, super::ModelError>
    where
        F: Future,
    {
        let Some(remaining) = self.timeout.checked_sub(self.last_activity.elapsed()) else {
            return Err(stream_idle_timeout(self.timeout));
        };
        tokio::time::timeout(remaining, future)
            .await
            .map_err(|_| stream_idle_timeout(self.timeout))
    }

    pub(crate) fn record_activity(&mut self) {
        self.last_activity = Instant::now();
    }
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
        .map_err(|_| stream_idle_timeout(idle_timeout))
}

fn stream_idle_timeout(timeout: Duration) -> super::ModelError {
    super::ModelError::StreamIdleTimeout { timeout }
}

#[cfg(test)]
#[path = "stream_timeout_tests.rs"]
mod tests;
