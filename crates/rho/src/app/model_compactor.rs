//! Host compactor. Tiers, cheapest first: elide old tool results, then
//! provider-native compaction, then a text summary.
//!
//! A text summary normally comes from the session model and resends the
//! session's own history, tool specs, and prompt cache key, so the provider
//! serves most of the request from the cache the session's last turn wrote. A configured summarizer model (`[internal_agents.compaction]`)
//! cannot share that cache and gets a rendered transcript instead.

use std::sync::Arc;

use rho_sdk::{
    model::{
        context::estimate_context_tokens, Message, ModelIdentity, ModelRequest, ModelResponse,
        ModelUsage, ToolSpec,
    },
    provider::{ModelProvider, ModelRequestOptions},
    CompactionFuture, CompactionOutput, CompactionRequest, Compactor, ContextEstimate, Error,
    ProviderRequestOutcome, ProviderRequestUsageContext, ProviderRequestUsageEvent,
    ProviderRequestUsageRecording,
};

use crate::{
    compaction::{
        build_session_summary_request, build_summary_request_messages, elide_tool_results,
        partition_messages_for_compaction, summary_replacement, summary_reserve_tokens,
        CompactionConfig, CompactionPartition, SummarizerModel,
    },
    diagnostics::{CompactionTier, CompactionTierReport, RuntimeDiagnostics},
    session::recall::RecallStore,
};

pub(crate) struct ModelCompactor {
    pub(super) provider: Arc<dyn ModelProvider>,
    pub(super) usage_recording: ProviderRequestUsageRecording,
    pub(super) tool_specs: Vec<ToolSpec>,
    pub(super) reasoning: rho_sdk::ReasoningLevel,
    pub(super) config: CompactionConfig,
    pub(super) context_window: Option<u64>,
    pub(super) diagnostics: RuntimeDiagnostics,
    pub(super) recall: Option<RecallStore>,
    /// Built from `config.summarizer`; `None` summarizes with the session model.
    pub(super) summarizer: Option<Summarizer>,
}

/// The configured summarizer model. Its provider is built on first use and
/// kept for the compactor's lifetime; a config change rebuilds the compactor.
pub(super) struct Summarizer {
    model: SummarizerModel,
    provider: tokio::sync::OnceCell<Arc<dyn ModelProvider>>,
}

impl Summarizer {
    pub(super) fn new(model: SummarizerModel) -> Self {
        Self {
            model,
            provider: tokio::sync::OnceCell::new(),
        }
    }

    /// A summarizer whose provider is already built.
    #[cfg(test)]
    pub(super) fn with_provider(model: SummarizerModel, provider: Arc<dyn ModelProvider>) -> Self {
        Self {
            model,
            provider: tokio::sync::OnceCell::new_with(Some(provider)),
        }
    }

    async fn provider(&self) -> Result<&Arc<dyn ModelProvider>, Error> {
        self.provider
            .get_or_try_init(|| async {
                let SummarizerModel {
                    provider,
                    model,
                    auth,
                    reasoning,
                } = &self.model;
                crate::credential_store::build_provider(provider, model, *reasoning, auth)
                    .await
                    .map_err(|error| Error::InvalidConfiguration {
                        message: format!(
                            "could not build the compaction model {}: {error}",
                            rho_providers::provider::model_reference(provider, model)
                        ),
                    })
            })
            .await
    }
}

impl Compactor for ModelCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            let cancellation = request.cancellation().clone();
            let mut next_attempt_index = 1usize;

            // Tier 1: elide old tool results. Commit without a model request
            // when that alone reaches the target; otherwise later tiers see the
            // elided history.
            let context = request.context_estimate().unwrap_or_else(|| {
                ContextEstimate::from_estimated_tokens(estimate_context_tokens(
                    request.messages(),
                    &self.tool_specs,
                ))
            });
            let target_tokens = self.config.target_tokens_for_context(
                self.context_window,
                request.trigger(),
                context,
            );
            let (elided, elided_tool_results) = match self.elide(&request, target_tokens) {
                Some(elision) => (Some(elision.messages), elision.originals.len()),
                None => (None, 0),
            };
            let report = |tier| {
                self.diagnostics
                    .record_compaction_tier(CompactionTierReport {
                        tier,
                        elided_tool_results,
                    })
            };
            if let Some(elided) = &elided {
                let tokens = estimate_context_tokens(elided, &self.tool_specs);
                if tokens <= target_tokens {
                    report(CompactionTier::Elision);
                    return CompactionOutput::new(elided.clone());
                }
            }
            let messages = elided.as_deref().unwrap_or(request.messages());

            match self
                .try_native_compaction(
                    messages,
                    request.service_tier(),
                    cancellation.clone(),
                    usage_context(&request, self.provider.identity()),
                    &mut next_attempt_index,
                )
                .await
            {
                NativeCompactionResult::Success(output) => {
                    report(CompactionTier::Native);
                    return Ok(output);
                }
                NativeCompactionResult::Cancelled => return Err(Error::Cancelled),
                // Explicit fallback to portable text-summary compaction.
                NativeCompactionResult::Unavailable | NativeCompactionResult::Failed => {}
            }

            let Some(partition) =
                partition_messages_for_compaction(messages, &self.tool_specs, target_tokens)
            else {
                report(match elided {
                    Some(_) => CompactionTier::Elision,
                    None => CompactionTier::Unchanged,
                });
                return CompactionOutput::new(messages.to_vec());
            };
            let summary = self
                .summarize(
                    &request,
                    &partition,
                    SummaryBudget {
                        context,
                        target_tokens,
                    },
                    &mut next_attempt_index,
                )
                .await?;
            self.diagnostics
                .record_compaction_tier(CompactionTierReport {
                    tier: CompactionTier::TextSummary,
                    elided_tool_results: if summary.from_elided {
                        elided_tool_results
                    } else {
                        0
                    },
                });
            CompactionOutput::with_usage(summary.replacement, summary.usage)
        })
    }

    fn cancellation_mode(&self) -> rho_sdk::CompactorCancellationMode {
        rho_sdk::CompactorCancellationMode::Cooperative
    }
}

#[derive(Debug)]
enum NativeCompactionResult {
    Success(CompactionOutput),
    Failed,
    Unavailable,
    Cancelled,
}

/// Context accounting a text summary is sized against.
#[derive(Clone, Copy)]
struct SummaryBudget {
    /// Accounting for the uncompacted request history.
    context: ContextEstimate,
    /// Post-compaction target, in local-estimator units.
    target_tokens: u64,
}

struct Summary {
    replacement: Vec<Message>,
    usage: ModelUsage,
    /// Whether the summary was written from the elided history.
    from_elided: bool,
}

/// One way to ask for a text summary. [`ModelCompactor::summarize`] tries
/// plans in order and falls through to the next on any failure other than
/// cancellation.
struct SummaryPlan<'a> {
    label: &'static str,
    /// `None` means the configured summarizer, built on first use.
    provider: Option<&'a dyn ModelProvider>,
    messages: Vec<Message>,
    /// The partition `messages` summarizes; the replacement is built around it.
    partition: CompactionPartition<'a>,
    tools: &'a [ToolSpec],
    reasoning: rho_sdk::ReasoningLevel,
    prompt_cache_key: Option<&'a str>,
    from_elided: bool,
}

impl ModelCompactor {
    /// Elides only when the agent can recall, and only after the originals are
    /// saved. A failed save skips elision rather than stranding a stub.
    fn elide(
        &self,
        request: &CompactionRequest,
        target_tokens: u64,
    ) -> Option<crate::compaction::Elision> {
        let dir = self.recall.as_ref()?.dir()?;
        let elision = elide_tool_results(request.messages(), &self.tool_specs, target_tokens)?;
        match crate::session::recall::save(&dir, &elision.originals) {
            Ok(()) => Some(elision),
            Err(error) => {
                tracing::warn!(%error, "could not save elided tool results; skipping elision");
                None
            }
        }
    }

    /// Writes the text summary around `partition`, which covers the history
    /// after elision.
    ///
    /// Plans, in order:
    /// - The configured summarizer, if any, on the rendered transcript.
    /// - The session model on the session's own unelided history, tools,
    ///   reasoning, service tier, and cache key, so the provider serves the
    ///   prefix from its cache. Skipped when a summarizer is configured, when
    ///   recovering from overflow, or when it would not fit the window.
    /// - The session model on the rendered transcript.
    ///
    /// Any failure except cancellation moves on to the next plan, so a broken
    /// summarizer or a provider that rejects the cache-shaped request still
    /// compacts. A tool call in a reply is a failed summary and never runs.
    async fn summarize<'a>(
        &'a self,
        request: &'a CompactionRequest,
        partition: &CompactionPartition<'a>,
        budget: SummaryBudget,
        next_attempt_index: &mut usize,
    ) -> Result<Summary, Error> {
        let mut plans = Vec::new();
        if let Some(summarizer) = &self.summarizer {
            plans.push(SummaryPlan {
                label: "configured summarizer",
                provider: None,
                messages: build_summary_request_messages(partition),
                partition: partition.clone(),
                tools: &[],
                reasoning: summarizer.model.reasoning,
                prompt_cache_key: None,
                from_elided: true,
            });
        } else if let Some(plan) = self.session_history_plan(request, budget) {
            plans.push(plan);
        }
        plans.push(SummaryPlan {
            label: "session model transcript",
            provider: Some(self.provider.as_ref()),
            messages: build_summary_request_messages(partition),
            partition: partition.clone(),
            tools: &[],
            reasoning: self.reasoning,
            // The rendered transcript shares no prefix with the session.
            prompt_cache_key: None,
            from_elided: true,
        });

        let mut spent = ModelUsage::default();
        let mut plans = plans.into_iter().peekable();
        while let Some(plan) = plans.next() {
            let label = plan.label;
            match self
                .try_plan(request, plan, &mut spent, next_attempt_index)
                .await
            {
                Ok(summary) => return Ok(summary),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) if plans.peek().is_none() => return Err(error),
                Err(error) => {
                    tracing::warn!(%error, plan = label, "compaction summary failed; trying the next request");
                }
            }
        }
        unreachable!("the session model transcript plan is always present")
    }

    /// Runs one plan, adding every response's usage to `spent`.
    async fn try_plan(
        &self,
        request: &CompactionRequest,
        plan: SummaryPlan<'_>,
        spent: &mut ModelUsage,
        next_attempt_index: &mut usize,
    ) -> Result<Summary, Error> {
        let provider = match plan.provider {
            Some(provider) => provider,
            None => self
                .summarizer
                .as_ref()
                .expect("summarizer plans exist only with a summarizer")
                .provider()
                .await?
                .as_ref(),
        };
        let cancellation = request.cancellation().clone();
        let options = match request.service_tier() {
            Some(tier) => ModelRequestOptions::default().with_service_tier(tier),
            None => ModelRequestOptions::default(),
        };
        let result = crate::usage::send_recorded_with(
            provider,
            crate::usage::RecordedRequest {
                request: ModelRequest {
                    messages: &plan.messages,
                    tools: plan.tools,
                    cancellation: cancellation.clone(),
                    reasoning_level: plan.reasoning,
                    prompt_cache_key: plan.prompt_cache_key,
                },
                options,
                context: usage_context(request, provider.identity()),
                recording: self.usage_recording.clone(),
            },
            next_attempt_index,
            |_| {},
        )
        .await;
        let (ModelResponse::Assistant(blocks), usage) = match result {
            Ok(result) => result,
            Err(_) if cancellation.is_cancelled() => return Err(Error::Cancelled),
            Err(error) => return Err(error.into()),
        };
        *spent = spent.saturating_add(&usage);
        Ok(Summary {
            replacement: summary_replacement(&plan.partition, request.trigger(), &blocks)?,
            usage: spent.clone(),
            from_elided: plan.from_elided,
        })
    }

    /// The session-history plan, or `None` when recovering from overflow,
    /// when nothing would be summarized, or when the request would not leave
    /// room for the summary in the window. Overflow recovery skips it because
    /// the provider just rejected the context, so the window may be overstated.
    fn session_history_plan<'a>(
        &'a self,
        request: &'a CompactionRequest,
        budget: SummaryBudget,
    ) -> Option<SummaryPlan<'a>> {
        if request.trigger() == rho_sdk::CompactionTrigger::ContextOverflow {
            return None;
        }
        let tools = request.tool_specs().unwrap_or(&self.tool_specs);
        let partition =
            partition_messages_for_compaction(request.messages(), tools, budget.target_tokens)?;
        let messages = build_session_summary_request(request.messages(), &partition);
        let fits = self.context_window.is_none_or(|window| {
            let tokens =
                calibrated_tokens(estimate_context_tokens(&messages, tools), budget.context);
            tokens.saturating_add(summary_reserve_tokens(budget.target_tokens)) <= window
        });
        fits.then_some(SummaryPlan {
            label: "session model history",
            provider: Some(self.provider.as_ref()),
            messages,
            partition,
            tools,
            reasoning: self.reasoning,
            prompt_cache_key: request.prompt_cache_key(),
            from_elided: false,
        })
    }

    async fn try_native_compaction(
        &self,
        messages: &[Message],
        service_tier: Option<rho_sdk::model::ServiceTier>,
        cancellation: rho_sdk::CancellationToken,
        usage_context: ProviderRequestUsageContext,
        next_attempt_index: &mut usize,
    ) -> NativeCompactionResult {
        let model_request = ModelRequest {
            messages,
            // Native compact requests do not advertise tools.
            tools: &[],
            cancellation: cancellation.clone(),
            reasoning_level: self.reasoning,
            // No provider documents cache-key support for native compaction.
            prompt_cache_key: None,
        };
        let options = match service_tier {
            Some(tier) => ModelRequestOptions::default().with_service_tier(tier),
            None => ModelRequestOptions::default(),
        };
        let Some(future) = self
            .provider
            .native_compact_with_options(model_request, options)
        else {
            return NativeCompactionResult::Unavailable;
        };
        let response = future.await;
        let (result, failed_attempts) = response.into_parts();
        for attempt in failed_attempts {
            self.usage_recording
                .record(ProviderRequestUsageEvent::observed(
                    usage_context
                        .clone()
                        .with_attempt_index(*next_attempt_index),
                    attempt.usage,
                    ProviderRequestOutcome::Failed(attempt.kind),
                ))
                .await;
            *next_attempt_index += 1;
        }
        let outcome = match &result {
            Ok(_) => ProviderRequestOutcome::Completed,
            Err(_) if cancellation.is_cancelled() => ProviderRequestOutcome::Cancelled,
            Err(error) => ProviderRequestOutcome::Failed(error.kind()),
        };
        let usage = result
            .as_ref()
            .map(|output| output.usage().clone())
            .unwrap_or_default();
        self.usage_recording
            .record(ProviderRequestUsageEvent::observed(
                usage_context.with_attempt_index(*next_attempt_index),
                usage,
                outcome,
            ))
            .await;
        *next_attempt_index += 1;
        match result {
            Ok(output) => NativeCompactionResult::Success(output),
            Err(_) if cancellation.is_cancelled() => NativeCompactionResult::Cancelled,
            Err(_) => NativeCompactionResult::Failed,
        }
    }
}

/// Usage attribution for compaction requests sent by `identity`.
fn usage_context(
    request: &CompactionRequest,
    identity: ModelIdentity,
) -> ProviderRequestUsageContext {
    let mut context = ProviderRequestUsageContext::for_purpose(identity, "compaction");
    if let Some(session_id) = request.session_id() {
        context = context.with_session_id(session_id.clone());
    }
    if let Some(parent_session_id) = request.parent_session_id() {
        context = context.with_parent_session_id(parent_session_id.clone());
    }
    if let Some(run_id) = request.run_id() {
        context = context.with_run_id(run_id.clone());
    }
    if let Some(step_index) = request.step_index() {
        context = context.with_step_index(step_index);
    }
    if let Some(workspace_path) = request.workspace_path() {
        context = context.with_workspace_path(workspace_path.to_path_buf());
    }
    context
}

/// Scales a local estimate by how much larger the provider measured the
/// session's context than the local estimate. Never scales down.
fn calibrated_tokens(local: u64, context: ContextEstimate) -> u64 {
    let estimated = context.estimated_tokens();
    let measured = context.tokens();
    if estimated == 0 || measured <= estimated {
        return local;
    }
    let scaled = u128::from(local) * u128::from(measured) / u128::from(estimated);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "model_compactor_tests.rs"]
mod tests;
