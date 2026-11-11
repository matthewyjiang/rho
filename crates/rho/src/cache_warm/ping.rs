//! Cache-warm pings: one capped request that re-reads the session's cached
//! prompt prefix so the provider keeps it alive while the model idles on a
//! `process start` that asked for `keep_warm`.
//!
//! The scheduler in the parent module decides when; this module only builds a
//! request whose prefix matches what the next real turn sends:
//! session history (system prompt included), host request context, the
//! advertised tool specs, reasoning level, and prompt-cache key. A placeholder
//! user message ends it because some models reject assistant prefill.

use std::{num::NonZeroU32, sync::Arc};

use rho_sdk::{
    model::{Message, ModelUsage},
    provider::{ModelProvider, ModelRequestOptions},
    CancellationToken, ProviderRequestUsageContext, ProviderRequestUsageRecording, Session,
    SessionRequestPrefix,
};

/// Output cap for a ping. Receipt: the Responses API rejects
/// `max_output_tokens: 0` while streaming; 1 returned one output token, zero
/// reasoning tokens, and a full cache read (`/tmp/spike_resp.log`).
pub(crate) const PING_OUTPUT_TOKENS: NonZeroU32 = NonZeroU32::MIN;

/// Trailing user turn. Its text is never part of a real request, so only the
/// prefix before it matters for cache reads.
const PING_TEXT: &str = "(prompt cache keep-alive; no reply needed)";

/// Usage-ledger purpose for ping spend, separate from turns and compaction.
const PING_PURPOSE: &str = "cache_warm";

/// A ready-to-send ping, owned so it can run on a background task.
pub(crate) struct CacheWarmPing {
    provider: Arc<dyn ModelProvider>,
    prefix: SessionRequestPrefix,
    context: ProviderRequestUsageContext,
    recording: ProviderRequestUsageRecording,
}

/// What one ping cost and whether the provider failed it.
///
/// A capped ping normally ends as an `incomplete` response error after
/// reporting usage, so `error` only matters when no usage arrived.
#[derive(Debug, Default)]
pub(crate) struct CacheWarmOutcome {
    pub(crate) usage: ModelUsage,
    pub(crate) error: Option<String>,
}

impl CacheWarmPing {
    /// Builds a ping for `session`'s current prefix on `provider`, the model
    /// its next turn runs on.
    pub(crate) fn for_session(
        provider: Arc<dyn ModelProvider>,
        session: &Session,
        recording: ProviderRequestUsageRecording,
    ) -> Result<Self, rho_sdk::Error> {
        let prefix = session
            .request_prefix()?
            .with_trailing_message(Message::user_text(PING_TEXT));
        let context = ProviderRequestUsageContext::for_purpose(provider.identity(), PING_PURPOSE)
            .with_session_id(session.id().clone());
        Ok(Self {
            provider,
            prefix,
            context,
            recording,
        })
    }

    /// Sends the ping and records every attempt under the `cache_warm` purpose.
    pub(crate) async fn send(self) -> CacheWarmOutcome {
        let mut next_attempt_index = 0;
        let mut usage = ModelUsage::default();
        // No service tier: a priority multiplier would raise the ping price,
        // and a keep-alive needs no latency class.
        let options = ModelRequestOptions::default().with_max_output_tokens(PING_OUTPUT_TOKENS);
        let result = crate::usage::send_recorded_with(
            self.provider.as_ref(),
            crate::usage::RecordedRequest {
                request: self.prefix.as_request(CancellationToken::new()),
                options,
                context: self.context,
                recording: self.recording,
            },
            &mut next_attempt_index,
            &mut usage,
            |_| {},
        )
        .await;
        CacheWarmOutcome {
            usage,
            error: result.err().map(|error| error.to_string()),
        }
    }
}
