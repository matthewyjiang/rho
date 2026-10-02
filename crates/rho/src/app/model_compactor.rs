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
    compaction_metrics::{
        removed_tool_calls, CompactionRecord, CompactionRunOutcome, CompactionTier, RecordIdentity,
        SummaryRequestPath,
    },
    diagnostics::RuntimeDiagnostics,
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
            let started = std::time::Instant::now();
            let occurred_at_ms = chrono::Utc::now().timestamp_millis();
            let mut trace = Trace::default();
            let result = self.compact_tiers(&request, &mut trace).await;
            let removed = match &result {
                Ok(output) => removed_tool_calls(request.messages(), output.messages()),
                Err(_) => Default::default(),
            };
            let record = CompactionRecord {
                identity: record_identity(&request, occurred_at_ms),
                outcome: CompactionRunOutcome::of(&result),
                trigger: request.trigger().into(),
                tier: trace.tier,
                request_path: trace.request_path,
                model: trace.model,
                elided_tool_results: trace.elided_tool_results,
                context_tokens: trace.context_tokens,
                prompt_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cost_usd_micros: None,
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                next_prompt_tokens: None,
                reread: None,
            }
            .with_usage(&trace.usage);
            self.diagnostics.record_compaction(record, removed);
            result
        })
    }

    fn cancellation_mode(&self) -> rho_sdk::CompactorCancellationMode {
        rho_sdk::CompactorCancellationMode::Cooperative
    }
}

/// What one compactor call did, filled in as tiers run.
#[derive(Default)]
struct Trace {
    tier: Option<CompactionTier>,
    request_path: Option<SummaryRequestPath>,
    model: Option<String>,
    elided_tool_results: usize,
    context_tokens: u64,
    /// Usage from every compaction request whose usage came back.
    usage: ModelUsage,
}

impl Trace {
    fn charge(&mut self, identity: &ModelIdentity, usage: &ModelUsage) {
        self.model = Some(rho_providers::provider::model_reference(
            &identity.provider,
            &identity.model,
        ));
        self.usage = self.usage.saturating_add(usage);
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
    path: SummaryRequestPath,
    /// `None` means the configured summarizer, built on first use.
    provider: Option<&'a dyn ModelProvider>,
    messages: Vec<Message>,
    /// The partition `messages` summarizes; the replacement is built around it.
    partition: CompactionPartition<'a>,
    tools: &'a [ToolSpec],
    reasoning: rho_sdk::ReasoningLevel,
    /// Only the session-history plan sends one. It has to match the session
    /// turn or the cache misses. Other plans share no prefix, and copying the
    /// session tier onto them can bill a summarizer or a transcript at the
    /// priority rate.
    service_tier: Option<rho_sdk::model::ServiceTier>,
    prompt_cache_key: Option<&'a str>,
    from_elided: bool,
}

impl ModelCompactor {
    /// Prefer the live advertised schemas; the snapshot is only for callers
    /// that did not supply a session tool projection.
    fn tool_specs<'a>(&'a self, request: &'a CompactionRequest) -> &'a [ToolSpec] {
        request.tool_specs().unwrap_or(&self.tool_specs)
    }

    /// Tier 1 elides old tool results and commits without a model request
    /// when that alone reaches the target; later tiers see the elided history.
    async fn compact_tiers(
        &self,
        request: &CompactionRequest,
        trace: &mut Trace,
    ) -> Result<CompactionOutput, Error> {
        let cancellation = request.cancellation().clone();
        let mut next_attempt_index = 1usize;
        let tools = self.tool_specs(request);
        let context = request.context_estimate().unwrap_or_else(|| {
            ContextEstimate::from_estimated_tokens(estimate_context_tokens(
                request.messages(),
                tools,
            ))
        });
        trace.context_tokens = context.tokens();
        let target_tokens =
            self.config
                .target_tokens_for_context(self.context_window, request.trigger(), context);
        let (elided, elided_tool_results) = match self.elide(request, target_tokens) {
            Some(elision) => (Some(elision.messages), elision.originals.len()),
            None => (None, 0),
        };
        trace.elided_tool_results = elided_tool_results;
        if let Some(elided) = &elided {
            let tokens = estimate_context_tokens(elided, tools);
            if tokens <= target_tokens {
                trace.tier = Some(CompactionTier::Elision);
                return CompactionOutput::new(elided.clone());
            }
        }
        let messages = elided.as_deref().unwrap_or(request.messages());

        match self
            .try_native_compaction(
                messages,
                request.service_tier(),
                cancellation.clone(),
                usage_context(request, self.provider.identity()),
                &mut next_attempt_index,
                trace,
            )
            .await
        {
            NativeCompactionResult::Success(output) => {
                trace.tier = Some(CompactionTier::Native);
                return Ok(output);
            }
            NativeCompactionResult::Cancelled => {
                trace.tier = Some(CompactionTier::Native);
                return Err(Error::Cancelled);
            }
            // Explicit fallback to portable text-summary compaction.
            NativeCompactionResult::Unavailable | NativeCompactionResult::Failed => {}
        }

        let Some(partition) = partition_messages_for_compaction(messages, tools, target_tokens)
        else {
            trace.tier = Some(match elided {
                Some(_) => CompactionTier::Elision,
                None => CompactionTier::Unchanged,
            });
            return CompactionOutput::new(messages.to_vec());
        };
        trace.tier = Some(CompactionTier::TextSummary);
        let summary = self
            .summarize(
                request,
                &partition,
                SummaryBudget {
                    context,
                    target_tokens,
                },
                &mut next_attempt_index,
                trace,
            )
            .await?;
        if !summary.from_elided {
            trace.elided_tool_results = 0;
        }
        CompactionOutput::with_usage(summary.replacement, summary.usage)
    }

    /// Elides only when the agent can recall, and only after the originals are
    /// saved. A failed save skips elision rather than stranding a stub.
    fn elide(
        &self,
        request: &CompactionRequest,
        target_tokens: u64,
    ) -> Option<crate::compaction::Elision> {
        let dir = self.recall.as_ref()?.dir()?;
        let elision =
            elide_tool_results(request.messages(), self.tool_specs(request), target_tokens)?;
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
    ///   prefix from its cache. Also the fallback when the summarizer fails,
    ///   so a bad summarizer does not cost more than no summarizer. Skipped
    ///   when recovering from overflow, or when it would not fit the window.
    /// - The session model on the rendered transcript, with no service tier.
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
        trace: &mut Trace,
    ) -> Result<Summary, Error> {
        let mut plans = Vec::new();
        if let Some(summarizer) = &self.summarizer {
            plans.push(SummaryPlan {
                path: SummaryRequestPath::Summarizer,
                provider: None,
                messages: build_summary_request_messages(partition),
                partition: partition.clone(),
                tools: &[],
                reasoning: summarizer.model.reasoning,
                service_tier: None,
                prompt_cache_key: None,
                from_elided: true,
            });
        }
        if let Some(plan) = self.session_history_plan(request, budget) {
            plans.push(plan);
        }
        plans.push(SummaryPlan {
            path: SummaryRequestPath::Transcript,
            provider: Some(self.provider.as_ref()),
            messages: build_summary_request_messages(partition),
            partition: partition.clone(),
            tools: &[],
            reasoning: self.reasoning,
            service_tier: None,
            // The rendered transcript shares no prefix with the session.
            prompt_cache_key: None,
            from_elided: true,
        });

        let mut spent = ModelUsage::default();
        let mut plans = plans.into_iter().peekable();
        while let Some(plan) = plans.next() {
            let path = plan.path;
            trace.request_path = Some(path);
            match self
                .try_plan(request, plan, &mut spent, next_attempt_index, trace)
                .await
            {
                Ok(summary) => return Ok(summary),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) if plans.peek().is_none() => return Err(error),
                Err(error) => {
                    tracing::warn!(%error, plan = path.label(), "compaction summary failed; trying the next request");
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
        trace: &mut Trace,
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
        let options = match plan.service_tier {
            Some(tier) => ModelRequestOptions::default().with_service_tier(tier),
            None => ModelRequestOptions::default(),
        };
        let mut reported = ModelUsage::default();
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
            &mut reported,
            |_| {},
        )
        .await;
        // Every attempt, including failed ones and internal retries, is
        // compaction cost even when the plan fails.
        trace.charge(&provider.identity(), &reported);
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
        let tools = self.tool_specs(request);
        let partition =
            partition_messages_for_compaction(request.messages(), tools, budget.target_tokens)?;
        let messages = build_session_summary_request(request.messages(), &partition);
        let fits = self.context_window.is_none_or(|window| {
            let tokens =
                calibrated_tokens(estimate_context_tokens(&messages, tools), budget.context);
            tokens.saturating_add(summary_reserve_tokens(budget.target_tokens)) <= window
        });
        fits.then_some(SummaryPlan {
            path: SummaryRequestPath::SessionHistory,
            provider: Some(self.provider.as_ref()),
            messages,
            partition,
            tools,
            reasoning: self.reasoning,
            service_tier: request.service_tier(),
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
        trace: &mut Trace,
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
        let identity = self.provider.identity();
        for attempt in failed_attempts {
            trace.charge(&identity, &attempt.usage);
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
        trace.charge(&identity, &usage);
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

/// Ledger identity for the record of `request`.
fn record_identity(request: &CompactionRequest, occurred_at_ms: i64) -> RecordIdentity {
    RecordIdentity {
        event_id: uuid::Uuid::new_v4().to_string(),
        occurred_at_ms,
        session_id: request.session_id().map(ToString::to_string),
        parent_session_id: request.parent_session_id().map(ToString::to_string),
        run_id: request.run_id().map(ToString::to_string),
        workspace_path: request
            .workspace_path()
            .map(|path| path.to_string_lossy().into_owned()),
    }
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

#[cfg(test)]
#[path = "model_compactor_session_tests.rs"]
mod session_tests;
