//! Cache-warm pings: one capped request that re-reads the session's cached
//! prompt prefix so the provider keeps it alive while the model idles on a
//! `process start` that asked for `keep_warm`.
//!
//! The TUI owns scheduling and stop policy (`tui/cache_warm.rs`); this module
//! only builds a request whose prefix matches what the next real turn sends:
//! session history (system prompt included), host request context, the
//! advertised tool specs, reasoning level, and prompt-cache key. A placeholder
//! user message ends it because some models reject assistant prefill.

use std::{num::NonZeroU32, sync::Arc, time::Duration};

use rho_sdk::{
    model::{Message, ModelRequest, ModelUsage, ToolSpec},
    provider::{ModelProvider, ModelRequestOptions},
    CancellationToken, ProviderRequestUsageContext, ProviderRequestUsageRecording, ReasoningLevel,
};

use super::InteractiveRuntime;

/// Output cap for a ping. Receipt: the Responses API rejects
/// `max_output_tokens: 0` while streaming; 1 returned one output token, zero
/// reasoning tokens, and a full cache read (`/tmp/spike_resp.log`).
const PING_OUTPUT_TOKENS: NonZeroU32 = NonZeroU32::MIN;

/// Trailing user turn. Its text is never part of a real request, so only the
/// prefix before it matters for cache reads.
const PING_TEXT: &str = "(prompt cache keep-alive; no reply needed)";

/// Usage-ledger purpose for ping spend, separate from turns and compaction.
const PING_PURPOSE: &str = "cache_warm";

/// A ready-to-send ping, owned so it can run on a background task.
pub(crate) struct CacheWarmPing {
    provider: Arc<dyn ModelProvider>,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    reasoning: ReasoningLevel,
    prompt_cache_key: String,
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
                request: ModelRequest {
                    messages: &self.messages,
                    tools: &self.tools,
                    cancellation: CancellationToken::new(),
                    reasoning_level: self.reasoning,
                    prompt_cache_key: Some(&self.prompt_cache_key),
                },
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

impl InteractiveRuntime {
    /// True while a live process holds the prompt cache warm.
    pub(crate) fn has_keep_warm_hold(&self) -> bool {
        self.processes()
            .is_some_and(crate::tools::process::ProcessManager::has_keep_warm_hold)
    }

    /// Whether the current model route can send a capped ping.
    pub(crate) fn supports_cache_warm(&self) -> bool {
        self.provider.provider().honors_max_output_tokens()
    }

    /// Prompt cache TTL configured for the current provider, if any.
    pub(crate) fn prompt_cache_ttl(&self) -> Option<Duration> {
        self.config
            .providers
            .prompt_cache_ttl(&self.provider.provider().identity().provider)
    }

    /// Builds a ping for the current session prefix.
    pub(crate) fn cache_warm_ping(&self) -> CacheWarmPing {
        let provider = Arc::clone(self.provider.provider());
        let session_id = self.session_id().clone();
        let mut messages = self.sessions.history();
        messages.extend(rho_sdk::RequestContext::messages(
            &self.tools.todo_state(),
            &session_id,
        ));
        messages.push(Message::user_text(PING_TEXT));
        let context = ProviderRequestUsageContext::for_purpose(provider.identity(), PING_PURPOSE)
            .with_session_id(session_id.clone());
        CacheWarmPing {
            messages,
            tools: self.tools.specs(),
            reasoning: self.sessions.session().reasoning_level(),
            prompt_cache_key: super::startup::prompt_cache_key(session_id.as_str()),
            context,
            recording: self.usage_recording.clone(),
            provider,
        }
    }
}
