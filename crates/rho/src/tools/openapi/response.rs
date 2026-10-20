//! Reading a response and shaping the tool result.
//!
//! Scripts get `data = {status, body, next, headers}` for every HTTP answer,
//! errors included, so a script can branch on a 404. The model reads a short
//! text view bounded by `max_output_bytes`.

use std::collections::BTreeMap;

use reqwest::header::{HeaderMap, CONTENT_LENGTH, CONTENT_TYPE, LINK};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use url::Url;

/// Largest response body one call reads, the `max_response_bytes` limit, a
/// tripwire against runaway downloads. GitHub's largest list pages at
/// `per_page=100` measured 1.9 MB (pulls), 0.7 MB (issues), 0.6 MB (org repos),
/// and 0.5 MB (commits), so real pages sit at half the cap or less.
///
/// A codemode script holds the parsed body as Starlark values, which measured
/// 2.4x to 3.0x the body's bytes (pulls 1.89 MB became 4.45 MB). The 8 MiB
/// script heap therefore fits one body of about 2.7 MB; a larger body that
/// passes this cap fails the script with the heap limit instead.
pub(crate) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Bytes of a non-2xx body quoted in the one-line error, as opencode does.
const ERROR_BODY_PREVIEW_BYTES: usize = 1024;

/// What a call returns to scripts.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct CallResult {
    /// HTTP status code.
    pub(crate) status: u16,
    /// Parsed JSON, the text of a non-JSON body, or null when empty.
    pub(crate) body: Value,
    /// Arguments for the next page, from a `Link: <...>; rel="next"` header.
    /// Null on the last page.
    pub(crate) next: Option<Value>,
    /// Rate-limit, `retry-after`, and total-count headers, lowercased.
    pub(crate) headers: BTreeMap<String, String>,
}

impl CallResult {
    /// Whether the status is 2xx; anything else is a failed call.
    pub(crate) fn succeeded(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Why a response body could not be read.
#[derive(Debug)]
pub(crate) enum BodyError {
    TooLarge { size: u64, limit: usize },
    Transport(reqwest::Error),
}

/// Read the whole body, failing once it passes `limit`. An oversized body
/// without a `Content-Length` is drained to report its real size; the call
/// timeout still bounds that.
pub(crate) async fn read_body(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, BodyError> {
    let declared = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
    if let Some(size) = declared.filter(|size| *size > limit as u64) {
        return Err(BodyError::TooLarge { size, limit });
    }
    let mut body = Vec::new();
    let mut size = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Transport)? {
        size += chunk.len() as u64;
        if size <= limit as u64 {
            body.extend_from_slice(&chunk);
        } else {
            body = Vec::new();
        }
    }
    if size > limit as u64 {
        return Err(BodyError::TooLarge { size, limit });
    }
    Ok(body)
}

/// Parse `bytes` as the body a script sees.
pub(crate) fn decode_body(headers: &HeaderMap, bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    let essence = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());
    let maybe_json = match essence.as_deref() {
        None => true,
        Some(essence) => essence == "application/json" || essence.ends_with("+json"),
    };
    if maybe_json {
        if let Ok(value) = serde_json::from_slice(bytes) {
            return value;
        }
    }
    Value::String(String::from_utf8_lossy(bytes).into_owned())
}

/// The headers scripts need to pace and page through an API.
pub(crate) fn kept_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    const KEPT: &[&str] = &[
        "retry-after",
        "x-total-count",
        "x-total",
        "ratelimit",
        "ratelimit-policy",
        "ratelimit-limit",
        "ratelimit-remaining",
        "ratelimit-reset",
    ];
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            KEPT.contains(&name) || name.starts_with("x-ratelimit-")
        })
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect()
}

/// The `rel="next"` target of the `Link` headers, resolved against `base`.
pub(crate) fn next_link(headers: &HeaderMap, base: &Url) -> Option<Url> {
    headers
        .get_all(LINK)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(next_target)
        .and_then(|target| base.join(target).ok())
}

/// Find the `rel="next"` target in one `Link` header value, such as
/// `<https://api.example.com/items?page=2>; rel="next", <...>; rel="last"`.
fn next_target(value: &str) -> Option<&str> {
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let close = after.find('>')?;
        let target = &after[..close];
        // Parameters run to the next `,` outside a quoted string, so a
        // `title="a; rel=next"` is one parameter, not a rel.
        let mut params = Vec::new();
        let mut start = close + 1;
        let mut end = after.len();
        let mut quoted = false;
        let mut escaped = false;
        for (index, character) in after
            .char_indices()
            .skip_while(|(index, _)| *index <= close)
        {
            if escaped {
                escaped = false;
                continue;
            }
            match character {
                // A quoted-pair such as `\"` stays inside the string.
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                ';' if !quoted => {
                    params.push(&after[start..index]);
                    start = index + 1;
                }
                ',' if !quoted => {
                    end = index;
                    break;
                }
                _ => {}
            }
        }
        params.push(&after[start..end]);
        let is_next = params.iter().any(|param| {
            param.split_once('=').is_some_and(|(key, value)| {
                key.trim().eq_ignore_ascii_case("rel")
                    && value
                        .trim()
                        .trim_matches('"')
                        .split_ascii_whitespace()
                        .any(|rel| rel.eq_ignore_ascii_case("next"))
            })
        });
        if is_next {
            return Some(target);
        }
        rest = &after[end..];
    }
    None
}

/// The model's view of a result, at most `max_bytes` long. A non-2xx answer
/// becomes one line with a short quote of the body.
pub(crate) fn model_text(route: &str, result: &CallResult, max_bytes: usize) -> String {
    let reason = reqwest::StatusCode::from_u16(result.status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or("");
    let status = format!("{} {reason}", result.status);
    let body = match &result.body {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        body => body.to_string(),
    };
    let text = if result.succeeded() {
        let mut text = format!("HTTP {}", status.trim_end());
        if !body.is_empty() {
            text.push('\n');
            text.push_str(&body);
        }
        if let Some(next) = &result.next {
            text.push_str(&format!("\nnext page arguments: {next}"));
        }
        text
    } else {
        let preview = rho_tools::tool::truncate(
            body.split_whitespace().collect::<Vec<_>>().join(" "),
            ERROR_BODY_PREVIEW_BYTES,
        );
        format!("{route} returned {}: {preview}", status.trim_end())
            .trim_end_matches([' ', ':'])
            .to_owned()
    };
    rho_tools::tool::truncate(
        text,
        max_bytes.saturating_sub(rho_tools::tool::TRUNCATION_MARKER.len()),
    )
}

#[cfg(test)]
#[path = "response_tests.rs"]
mod tests;
