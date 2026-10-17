//! The HTTP exchange shared by Rho's decision-model clients
//! ([`crate::system_one`], [`crate::openai_decisions`]): one JSON POST with an
//! optional bearer key, cancellable, whose errors never carry response text or
//! the URL, either of which a server or proxy may fill with credentials.

use std::time::Duration;

use rho_sdk::{decision::DecisionError, CancellationToken, SecretString};
use url::Url;

/// An HTTP client for decision requests that gives up after `timeout`.
pub(crate) fn client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    crate::tls::reqwest_client_builder()
        .timeout(timeout)
        .build()
        .map_err(reqwest::Error::without_url)
}

/// POSTs `body` as JSON to `url` and returns the success response's text.
/// A non-success status is [`DecisionError::Status`], without the body.
pub(crate) async fn post_json(
    client: &reqwest::Client,
    url: &Url,
    api_key: Option<&SecretString>,
    body: Vec<u8>,
    cancellation: &CancellationToken,
) -> Result<String, DecisionError> {
    let response = async {
        let mut post = client
            .post(url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(api_key) = api_key {
            post = post.bearer_auth(api_key.expose_secret());
        }
        let mut request = post.build().map_err(transport_error)?;
        crate::providers::request_body::encode_request(&mut request)
            .map_err(|error| DecisionError::Model(Box::new(error)))?;
        let response = client.execute(request).await.map_err(transport_error)?;
        let status = response.status();
        let text = response.text().await.map_err(transport_error)?;
        Ok::<_, DecisionError>((status, text))
    };
    let (status, text) = tokio::select! {
        () = cancellation.cancelled() => return Err(DecisionError::Cancelled),
        response = response => response?,
    };
    if !status.is_success() {
        return Err(DecisionError::Status {
            status: status.as_u16(),
        });
    }
    Ok(text)
}

fn transport_error(error: reqwest::Error) -> DecisionError {
    DecisionError::Model(Box::new(error.without_url()))
}
