use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use tokio::time::{sleep, Instant};

const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

/// RFC 8628 device-authorization start payload shared by xAI, Kimi, and GitHub.
#[derive(Deserialize)]
pub(super) struct DeviceCodeResponse {
    pub device_code: Option<String>,
    pub user_code: Option<String>,
    pub verification_uri: Option<String>,
    pub verification_uri_complete: Option<String>,
    pub expires_in: Option<u64>,
    pub interval: Option<u64>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum FirstPoll {
    Immediate,
    AfterInterval,
}

#[derive(Clone, Copy)]
pub(super) enum DevicePollHttp {
    /// Deserialize JSON for success and error statuses.
    Json,
    /// Fail HTTP errors before JSON.
    ErrorForStatus,
}

pub(super) struct DevicePollRequest<'a> {
    pub client: &'a reqwest::Client,
    pub endpoint: &'a str,
    pub form: &'a [(&'a str, &'a str)],
    pub extra_headers: &'a [(&'a str, &'a str)],
    pub expires_in: Duration,
    pub interval: Duration,
    pub first_poll: FirstPoll,
    pub http: DevicePollHttp,
}

pub(super) enum DevicePollStep<T> {
    Tokens(T),
    Pending,
    SlowDown,
    Denied { description: String },
    Timeout,
    Fatal { error: String },
}

pub(super) enum DevicePollOutcome<T> {
    Tokens(T),
    Denied { description: String },
    Timeout,
    Fatal { error: String },
}

/// Shared pending / slow_down / expired_token mapping for RFC 8628 poll errors.
pub(super) fn standard_device_poll_step<T>(error: Option<&str>) -> Option<DevicePollStep<T>> {
    match error {
        Some("authorization_pending") => Some(DevicePollStep::Pending),
        Some("slow_down") => Some(DevicePollStep::SlowDown),
        Some("expired_token") => Some(DevicePollStep::Timeout),
        _ => None,
    }
}

/// Poll a device-code token endpoint until tokens, denial, timeout, or a fatal error.
pub(super) async fn poll_device_token<T, F>(
    request: DevicePollRequest<'_>,
    interpret: F,
) -> Result<DevicePollOutcome<T>, reqwest::Error>
where
    T: serde::de::DeserializeOwned,
    F: Fn(StatusCode, T) -> DevicePollStep<T>,
{
    let deadline = Instant::now() + request.expires_in;
    let mut interval = request.interval;
    loop {
        if Instant::now() >= deadline {
            return Ok(DevicePollOutcome::Timeout);
        }
        if matches!(request.first_poll, FirstPoll::AfterInterval) {
            sleep(interval).await;
            if Instant::now() >= deadline {
                return Ok(DevicePollOutcome::Timeout);
            }
        }

        let mut builder = request.client.post(request.endpoint).form(request.form);
        for &(name, value) in request.extra_headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await?;
        let (status, body) = match request.http {
            DevicePollHttp::Json => {
                let status = response.status();
                let body = response.json::<T>().await?;
                (status, body)
            }
            DevicePollHttp::ErrorForStatus => {
                let response = response.error_for_status()?;
                let status = response.status();
                let body = response.json::<T>().await?;
                (status, body)
            }
        };

        match interpret(status, body) {
            DevicePollStep::Tokens(tokens) => return Ok(DevicePollOutcome::Tokens(tokens)),
            DevicePollStep::Pending => {}
            DevicePollStep::SlowDown => interval += SLOW_DOWN_INCREMENT,
            DevicePollStep::Denied { description } => {
                return Ok(DevicePollOutcome::Denied { description });
            }
            DevicePollStep::Timeout => return Ok(DevicePollOutcome::Timeout),
            DevicePollStep::Fatal { error } => return Ok(DevicePollOutcome::Fatal { error }),
        }

        if matches!(request.first_poll, FirstPoll::Immediate) {
            sleep(interval).await;
        }
    }
}
