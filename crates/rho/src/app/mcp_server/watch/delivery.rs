//! Delivering one piece of news to a webhook: the POST, what its answer
//! means, and when a failed one is tried again.
//!
//! Delivery is at least once: the watcher records news as delivered only
//! after a 2xx, so a crash between the two sends it again. A 2xx means the
//! receiver took it; for a bot routine, that one run was accepted and
//! billed. A timeout, a network error, 408, 429, or 5xx may pass, so they
//! are tried again on [`RetrySchedule`]; any other answer, a redirect
//! included, will not change by asking again, so it stops the watch at
//! once. Redirects are never followed: the secret goes only to the URL the
//! owner configured. The answer's body is never read.

use std::{fmt, time::Duration};

use serde_json::Value;

use crate::app::mcp_server::webhooks::WebhookTarget;

/// How long one POST may take, connecting included.
//
// The web tools' request timeout (`tools::web::util::HTTP_TIMEOUT`): a
// receiver that accepts a run answers at once, so this only bounds a stuck
// connection. The retry schedule counts gaps alone, so each attempt can
// add up to this much to it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The gap before the first retry, doubled for each one after.
//
// Long enough for a receiver's brief hiccup, short enough that one dropped
// packet delays news by seconds.
const FIRST_RETRY_GAP: Duration = Duration::from_secs(2);

/// The longest gap between retries.
//
// Once the gaps reach it, a failing delivery POSTs at most 6 times an
// hour, against a Grok routine's limit of 200 runs an hour (300 an hour
// per user): a receiver that keeps failing costs a small share of the
// budget the working watches share. Before that the gaps double from
// `FIRST_RETRY_GAP`: 2 s to 512 s, 9 retries in the first ~17 minutes
// (1022 s), 10 attempts counting the first.
const MAX_RETRY_GAP: Duration = Duration::from_secs(10 * 60);

/// How long the gaps between retries may add up to, the attempts' own time
/// aside.
//
// A routine's rate limit counts runs over an hour; one hour plus one
// longest gap outlasts a 429 for a full window. With the gaps above, the
// schedule is 15 attempts per piece of news: the 10 of the first ~17
// minutes, then 5 more 10 minutes apart, the last 4022 s (~67 min) of gaps
// after the first; a sixth would come after 4622 s, past the horizon. The
// gaps alone count: with each of the 15 attempts taking up to
// `REQUEST_TIMEOUT`, a receiver that never answers is given up on at most
// 4022 + 15 x 30 = 4472 s (~74.5 min) after the first attempt began, plus
// the re-check of the token and session before each attempt, a credential
// store read and one daemon round trip.
const RETRY_HORIZON: Duration = Duration::from_secs(70 * 60);

/// When a failed attempt is tried again: gaps doubling from `first_gap`
/// up to `max_gap`, while the gaps so far add up to at most `horizon`. The
/// gaps alone count, not the attempts' own time. Data, so tests run it
/// without waiting. [`Self::DEFAULT`] is a delivery's; a remote follow has
/// its own (`remote_follow.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetrySchedule {
    pub(super) first_gap: Duration,
    pub(super) max_gap: Duration,
    pub(super) horizon: Duration,
}

impl RetrySchedule {
    pub(super) const DEFAULT: Self = Self {
        first_gap: FIRST_RETRY_GAP,
        max_gap: MAX_RETRY_GAP,
        horizon: RETRY_HORIZON,
    };

    /// The gap before each retry, in order; it ends when the gaps would
    /// add up past the horizon.
    pub(super) fn gaps(self) -> impl Iterator<Item = Duration> {
        let mut gap = self.first_gap;
        let mut elapsed = Duration::ZERO;
        std::iter::from_fn(move || {
            elapsed = elapsed.saturating_add(gap);
            if elapsed > self.horizon {
                return None;
            }
            let this = gap;
            gap = gap.saturating_mul(2).min(self.max_gap);
            Some(this)
        })
    }
}

/// What one POST got back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Answer {
    /// The receiver answered with this HTTP status.
    Status(u16),
    /// No answer: the connection failed or timed out. Never holds the URL.
    Error(String),
}

impl fmt::Display for Answer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(formatter, "answer {status}"),
            Self::Error(error) => write!(formatter, "error: {error}"),
        }
    }
}

/// What to do after a POST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The receiver took it, with this status.
    Delivered(u16),
    /// It may pass: try again.
    Retry,
    /// Asking again will not change it: stop.
    Stall,
}

pub(super) fn verdict(answer: &Answer) -> Verdict {
    match *answer {
        Answer::Status(status @ 200..=299) => Verdict::Delivered(status),
        Answer::Status(408 | 429 | 500..=599) | Answer::Error(_) => Verdict::Retry,
        Answer::Status(_) => Verdict::Stall,
    }
}

/// POSTs news over HTTP.
pub(super) struct HttpPoster {
    client: reqwest::Client,
}

impl HttpPoster {
    pub(super) fn new() -> anyhow::Result<Self> {
        let client = crate::reqwest_client_builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("rho/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { client })
    }

    /// POSTs `body` to `target` with its bearer secret.
    pub(super) async fn post(&self, target: &WebhookTarget, body: &Value) -> Answer {
        let sent = self
            .client
            .post(&target.url)
            .bearer_auth(&target.secret)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await;
        match sent {
            // Dropped unread: the status is the whole answer.
            Ok(response) => Answer::Status(response.status().as_u16()),
            Err(error) => Answer::Error(describe(error.without_url())),
        }
    }
}

/// `error` with its causes, which name hosts at most, never the URL.
fn describe(error: reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
