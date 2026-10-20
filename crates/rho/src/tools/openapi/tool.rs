//! The tool that calls one read-only REST operation.
//!
//! Arguments are checked and the URL is built during preparation, before any
//! credential exists on the request, and the URL is what the policy
//! authorizes. Redirects are followed by hand because reqwest strips only
//! `Authorization` and cookies on a cross-host hop, while an API key header or
//! a `headers_from_env` header would ride along. Here every source header and
//! query credential goes only to the source origin; any other hop gets none.

use std::{sync::Arc, time::Duration};

use reqwest::{header::LOCATION, Method};
use rho_sdk::{
    model::ToolSpec,
    tool::{
        AuthorizedToolContext, OperationKind, PreparedToolInvocation, Tool, ToolError,
        ToolErrorKind, ToolInvocation, ToolMetadata, ToolOutput, ToolPreparationContext,
        ToolPrepareFuture, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget,
};
use serde_json::Value;
use url::{Origin, Url};

use super::{
    config::SourceHeaders,
    request::{PlannedRequest, RequestShape},
    response::{self, BodyError, CallResult},
};

/// Redirects one call follows, matching reqwest's default limit.
const MAX_REDIRECTS: usize = 10;

/// Sent on spec fetches and calls. Some APIs, GitHub's among them, reject
/// requests without a `User-Agent`, and reqwest sends none by default.
pub(crate) const USER_AGENT: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));

/// What every operation of one source shares.
#[derive(Debug)]
pub(crate) struct SourceClient {
    pub(crate) name: String,
    pub(crate) client: reqwest::Client,
    /// The only origin that receives `headers`.
    pub(crate) origin: Origin,
    pub(crate) headers: SourceHeaders,
    pub(crate) timeout: Duration,
    pub(crate) max_output_bytes: usize,
    pub(crate) max_response_bytes: usize,
}

impl SourceClient {
    /// The HTTP client for spec fetches and calls. Redirects are followed by
    /// hand; see the module docs.
    pub(crate) fn http_client() -> reqwest::Result<reqwest::Client> {
        crate::reqwest_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
    }
}

/// One operation as a tool.
#[derive(Debug)]
pub(crate) struct OpenApiTool {
    pub(crate) spec: ToolSpec,
    pub(crate) source: Arc<SourceClient>,
    pub(crate) method: Method,
    /// `GET /repos/{owner}/{repo}`, for messages.
    pub(crate) route: String,
    pub(crate) base: Url,
    pub(crate) path: String,
    pub(crate) shape: RequestShape,
    /// The media type the operation's success response declares.
    pub(crate) accept: Option<String>,
}

impl Tool for OpenApiTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([CapabilityKind::Network])
    }

    fn output_schema(&self) -> Option<Value> {
        Some(rho_tools::output_schema::<CallResult>())
    }

    fn start_metadata(&self, _arguments: &Value) -> ToolMetadata {
        ToolMetadata::new().operation(OperationKind::Network)
    }

    fn prepare<'a>(
        &'a self,
        invocation: ToolInvocation,
        _context: ToolPreparationContext,
    ) -> ToolPrepareFuture<'a> {
        let arguments = invocation.into_arguments();
        Box::pin(async move {
            let planned = self
                .shape
                .build(&self.base, &self.path, &arguments)
                .map_err(|error| ToolError::new(ToolErrorKind::InvalidArguments, error))?;
            let capability = CapabilityRequest::network(
                NetworkTarget::Url(planned.url.to_string()),
                CapabilitySource::built_in_tool(self.spec.name.clone()),
            );
            Ok(PreparedToolInvocation::resource_aware(
                [],
                [capability],
                ToolMetadata::new().operation(OperationKind::Network),
                move |context| Box::pin(self.execute(planned, arguments, context)),
            ))
        })
    }
}

impl OpenApiTool {
    async fn execute(
        &self,
        planned: PlannedRequest,
        arguments: Value,
        context: AuthorizedToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let timeout = self.source.timeout;
        let result = tokio::select! {
            result = self.exchange(planned, &arguments) => result?,
            () = context.cancellation().cancelled() => return Err(ToolError::cancelled()),
            () = tokio::time::sleep(timeout) => {
                return Err(self.failure(format!(
                    "timed out after {}s (timeout_secs)",
                    timeout.as_secs()
                )));
            }
        };
        let text = response::model_text(&self.route, &result, self.source.max_output_bytes);
        let failed = !result.succeeded();
        rho_tools::Rendered::new(text, result)
            .failed_if(failed)
            .into_tool_output(ToolMetadata::new().operation(OperationKind::Network))
    }

    /// Send the request, follow redirects, and read the final answer.
    async fn exchange(
        &self,
        planned: PlannedRequest,
        arguments: &Value,
    ) -> Result<CallResult, ToolError> {
        let PlannedRequest { url, headers, body } = planned;
        // Only the first URL was authorized, so redirects stay on its origin.
        let authorized = url.origin();
        // Source headers and the query key go only to the source origin;
        // without a `base_url` the source has none.
        let trusted = authorized == self.source.origin;
        let mut sent = headers;
        if trusted {
            // Source headers go last, so an argument cannot replace them.
            for (name, value) in &self.source.headers.headers {
                sent.insert(name, value.clone());
            }
        }
        let mut url = url;
        for _ in 0..=MAX_REDIRECTS {
            let target = match (&self.source.headers.query, trusted) {
                (Some(credential), true) => with_query_credential(&url, credential),
                _ => url.clone(),
            };
            let mut request = self
                .source
                .client
                .request(self.method.clone(), target)
                .headers(sent.clone());
            if let Some(accept) = &self.accept {
                request = request.header(reqwest::header::ACCEPT, accept);
            }
            if let Some((media_type, bytes)) = &body {
                request = request
                    .header(reqwest::header::CONTENT_TYPE, media_type)
                    .body(bytes.clone());
            }
            let response = request
                .send()
                .await
                .map_err(|error| self.failure(error.without_url().to_string()))?;
            if response.status().is_redirection() {
                if let Some(location) = response.headers().get(LOCATION) {
                    let target = location
                        .to_str()
                        .ok()
                        .and_then(|location| url.join(location).ok())
                        .filter(|target| matches!(target.scheme(), "http" | "https"))
                        .ok_or_else(|| self.failure("redirected to an invalid location".into()))?;
                    if target.origin() != authorized {
                        return Err(self.failure(format!(
                            "could not follow redirect to {}: redirects to another origin are not followed",
                            target.origin().ascii_serialization()
                        )));
                    }
                    url = target;
                    continue;
                }
            }
            return self.read(response, &url, arguments).await;
        }
        Err(self.failure(format!("followed more than {MAX_REDIRECTS} redirects")))
    }

    async fn read(
        &self,
        response: reqwest::Response,
        url: &Url,
        arguments: &Value,
    ) -> Result<CallResult, ToolError> {
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = if self.method == Method::HEAD {
            Vec::new()
        } else {
            response::read_body(response, self.source.max_response_bytes)
                .await
                .map_err(|error| match error {
                    BodyError::TooLarge { size, limit } => self.failure(format!(
                        "response body is {size} bytes, over max_response_bytes ({limit} bytes)"
                    )),
                    BodyError::Transport(error) => self.failure(error.without_url().to_string()),
                })?
        };
        let credential = self.source.headers.query.as_ref().map(|(name, _)| name);
        let next = response::next_link(&headers, url).and_then(|next| {
            // A server may echo the query key into its links; it must not
            // come back to the model as an argument.
            let pairs: Vec<(String, String)> = next
                .query_pairs()
                .into_owned()
                .filter(|(name, _)| Some(name) != credential)
                .collect();
            self.shape.next_arguments(arguments, &pairs)
        });
        Ok(CallResult {
            status,
            body: response::decode_body(&headers, &bytes),
            next,
            headers: response::kept_headers(&headers),
        })
    }

    fn failure(&self, problem: String) -> ToolError {
        ToolError::new(
            ToolErrorKind::Execution,
            format!("{} ({}): {problem}", self.route, self.source.name),
        )
    }
}

/// `url` with the source's query key, replacing any same-named parameter so
/// an argument can neither override the key nor read it back.
fn with_query_credential(url: &Url, (name, value): &(String, String)) -> Url {
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != name)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut with_key = url.clone();
    with_key
        .query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .append_pair(name, value);
    with_key
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
