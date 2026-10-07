//! `rho __compaction_eval`: replays compaction on saved sessions and scores
//! what survives. A local development tool for #1314, driven by
//! `scripts/compaction_eval.py`; not part of CI.
//!
//! For each replay point it runs the production compactor once, asks each
//! probe from the post-compaction context, and scores the answer against the
//! uncompacted history. The references for a point do not depend on the
//! configuration, so variants are scored on the same questions. Transcripts go only to the configured models. Elided
//! originals go to a private temporary directory removed after each point,
//! and eval requests stay out of the usage ledger.

use std::{sync::Arc, time::Instant};

use rho_sdk::{
    model::{
        context::estimate_context_tokens, ContentBlock, Message, ModelRequest, ModelResponse,
        ModelUsage, ToolSpec,
    },
    provider::ModelProvider,
    CancellationToken, CompactionRequest, CompactionTrigger, Compactor, ContextEstimate,
    ProviderRequestUsageContext, ProviderRequestUsageRecording,
};
use serde::Serialize;

use super::{
    config_repository::ConfigRepository,
    runtime_builder::{build_compaction, CompactionSetup},
};
use crate::{
    cli::{Cli, CompactionEvalArgs, CompactionEvalTiers},
    compaction::{CompactionConfig, SummarizerModel},
    compaction_metrics::CompactionRecord,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    session::replay_points::{self, ReplayPoint},
};

#[path = "compaction_eval/probes.rs"]
mod probes;

use probes::{Probe, Reference, Scoring};

/// Bump when the report shape changes.
const REPORT_SCHEMA_VERSION: u32 = 3;
/// Points below this many estimated tokens are skipped: too little history
/// for a compaction to remove. Receipt: across 221 local sessions, the median
/// pre-compaction peak was 51k tokens and 131 sessions reached 32k.
const MIN_POINT_TOKENS: u64 = 32_768;

const ANSWER_SYSTEM_PROMPT: &str = "\
You are checking what an agent still knows after its context was compacted. \
Answer the question using only the conversation above. Do not call tools. If \
the conversation does not contain the answer, say you do not know. Be brief \
and exact.";

const JUDGE_SYSTEM_PROMPT: &str = "\
You grade whether an answer preserves the facts in a reference. A numbered \
reference is a list of items; otherwise it is one item. The answer may \
mention more than the reference; ignore extra content unless it contradicts \
the reference. Score the fraction of reference items the answer states \
correctly, from 0 to 1. Reply with a single JSON object and nothing else: \
{\"score\": <number>, \"reason\": \"<one sentence>\"}.";

pub(super) async fn run(args: &CompactionEvalArgs, cli: &Cli) -> anyhow::Result<()> {
    let setup = EvalSetup::load(args, cli).await?;
    let mut loaded = Vec::new();
    for path in &args.sessions {
        let (session_id, points) = replay_points::load(path)?;
        let selected = select_points(&points, args.points.get())
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        loaded.push((session_id, selected));
    }
    // Points are independent; `--jobs` caps how many replay at once so a
    // run does not trip provider rate limits.
    let permits = tokio::sync::Semaphore::new(args.jobs.get());
    let (setup, permits) = (&setup, &permits);
    let replays = loaded.iter().flat_map(|(session_id, points)| {
        points.iter().map(move |point| async move {
            let _permit = permits.acquire().await?;
            eprintln!(
                "compaction eval: session {session_id} node {} (~{} tokens)",
                point.node_id,
                estimate_context_tokens(&point.messages, &[])
            );
            setup.replay(point).await
        })
    });
    let mut replayed = futures_util::future::try_join_all(replays)
        .await?
        .into_iter();
    let sessions = loaded
        .iter()
        .map(|(session_id, points)| SessionReport {
            session_id: session_id.clone(),
            points: replayed.by_ref().take(points.len()).collect(),
        })
        .collect();
    let report = Report {
        schema_version: REPORT_SCHEMA_VERSION,
        probe_set_version: probes::PROBE_SET_VERSION,
        configuration: setup.describe(),
        sessions,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Up to `count` points at or above [`MIN_POINT_TOKENS`], spread evenly by
/// context size from the smallest to the largest.
fn select_points(points: &[ReplayPoint], count: usize) -> Vec<&ReplayPoint> {
    let eligible = points
        .iter()
        .filter(|point| estimate_context_tokens(&point.messages, &[]) >= MIN_POINT_TOKENS)
        .collect::<Vec<_>>();
    if eligible.len() <= count {
        return eligible;
    }
    let last = eligible.len() - 1;
    let mut chosen = (0..count)
        .map(|step| match count {
            1 => last,
            _ => step * last / (count - 1),
        })
        .collect::<Vec<_>>();
    chosen.dedup();
    chosen.into_iter().map(|index| eligible[index]).collect()
}

/// Everything a replay needs, resolved once.
struct EvalSetup {
    config: Config,
    compaction: CompactionConfig,
    tiers: CompactionEvalTiers,
    fixed_window: Option<u64>,
    session: Arc<dyn ModelProvider>,
    answerer: Arc<dyn ModelProvider>,
    judge: Arc<dyn ModelProvider>,
}

impl EvalSetup {
    async fn load(args: &CompactionEvalArgs, cli: &Cli) -> anyhow::Result<Self> {
        let config = load_eval_config(cli)?;

        let mut compaction = CompactionConfig::from(&config);
        if let Some(percent) = args.threshold_percent {
            compaction.threshold_percent = percent;
        }
        if let Some(percent) = args.target_percent {
            compaction.target_percent = percent;
        }
        anyhow::ensure!(
            compaction.target_percent < compaction.threshold_percent,
            "target percent {} must be below threshold percent {}",
            compaction.target_percent,
            compaction.threshold_percent
        );
        match args.summarizer.as_deref() {
            None => {}
            Some("session") => compaction.summarizer = None,
            Some(reference) => {
                let (provider, model) = split_reference(reference)?;
                compaction.summarizer = Some(SummarizerModel {
                    auth: default_auth(&config, provider),
                    provider: provider.into(),
                    model: model.into(),
                    reasoning: config.reasoning,
                });
            }
        }

        let session = build(&config, &config.provider, &config.model).await?;
        let answerer = match args.answer_model.as_deref() {
            Some(reference) => build_reference(&config, reference).await?,
            None => session.clone(),
        };
        let judge = match args.judge_model.as_deref() {
            Some(reference) => build_reference(&config, reference).await?,
            None => answerer.clone(),
        };
        // Native compaction output is provider state scoped to the model that
        // wrote it; another model sees only a portable notice, so scores
        // would measure the model mismatch rather than the compaction.
        anyhow::ensure!(
            args.tiers != CompactionEvalTiers::All || answerer.identity() == session.identity(),
            "--tiers all needs the answer model to be the session model: native \
             compaction output is readable only by the model that wrote it"
        );
        Ok(Self {
            config,
            compaction,
            tiers: args.tiers,
            fixed_window: args.context_window.map(std::num::NonZeroU64::get),
            session,
            answerer,
            judge,
        })
    }

    fn describe(&self) -> Configuration {
        let reference = |provider: &Arc<dyn ModelProvider>| {
            let identity = provider.identity();
            rho_providers::provider::model_reference(&identity.provider, &identity.model)
        };
        Configuration {
            session_model: reference(&self.session),
            reasoning: self.config.reasoning.to_string(),
            summarizer: self.compaction.summarizer.as_ref().map(|summarizer| {
                rho_providers::provider::model_reference(&summarizer.provider, &summarizer.model)
            }),
            tiers: match self.tiers {
                CompactionEvalTiers::All => "all",
                CompactionEvalTiers::Text => "text",
                CompactionEvalTiers::None => "none",
            },
            threshold_percent: self.compaction.threshold_percent,
            target_percent: self.compaction.target_percent,
            context_window: self.fixed_window,
            answer_model: reference(&self.answerer),
            judge_model: reference(&self.judge),
        }
    }

    /// Window for one point. Without `--context-window`, sized so the point
    /// sits exactly at the automatic threshold, as a live compaction would.
    fn window_for(&self, context_tokens: u64) -> u64 {
        self.fixed_window.unwrap_or_else(|| {
            (context_tokens * 100).div_ceil(u64::from(self.compaction.threshold_percent))
        })
    }

    async fn replay(&self, point: &ReplayPoint) -> anyhow::Result<PointReport> {
        let tools = replay_tool_specs(&point.messages);
        let before_tokens = estimate_context_tokens(&point.messages, &tools);
        let context_window = self.window_for(before_tokens);
        let (after, record) = match self.tiers {
            CompactionEvalTiers::None => (Ok(point.messages.clone()), None),
            CompactionEvalTiers::All | CompactionEvalTiers::Text => {
                self.compact(point, &tools, before_tokens, context_window)
                    .await?
            }
        };
        let after = match after {
            Ok(after) => after,
            Err(error) => {
                return Ok(PointReport {
                    node_id: point.node_id.clone(),
                    context_window,
                    before_tokens,
                    after_tokens: None,
                    summary: None,
                    compaction: record,
                    error: Some(error),
                    probes: Vec::new(),
                })
            }
        };
        let after_tokens = estimate_context_tokens(&after, &tools);
        let references = probes::references(&point.messages);
        let results = futures_util::future::join_all(
            references
                .iter()
                .map(|(probe, reference)| self.ask(*probe, reference, &after, &tools)),
        )
        .await;
        Ok(PointReport {
            node_id: point.node_id.clone(),
            context_window,
            before_tokens,
            after_tokens: Some(after_tokens),
            summary: after
                .iter()
                .find_map(Message::as_compaction_summary)
                .map(|summary| summary.text().to_owned()),
            compaction: record,
            error: None,
            probes: results,
        })
    }

    /// Runs the production compactor once on `point`. The outer error is a
    /// setup failure; the inner one is a failed compaction, which is reported.
    async fn compact(
        &self,
        point: &ReplayPoint,
        tools: &[ToolSpec],
        before_tokens: u64,
        context_window: u64,
    ) -> anyhow::Result<(Result<Vec<Message>, String>, Option<CompactionRecord>)> {
        let diagnostics = RuntimeDiagnostics::without_ledger(&self.config);
        // Live sessions elide into their recall folder. Replays elide into a
        // private temporary one so the elision tier runs as it would live.
        let recall_dir = tempfile::tempdir()?;
        let recall = crate::session::recall::RecallStore::default();
        recall.bind(Some(recall_dir.path().to_path_buf()));
        let provider = match self.tiers {
            CompactionEvalTiers::Text => Arc::new(NoNativeCompaction(self.session.clone())),
            CompactionEvalTiers::All | CompactionEvalTiers::None => self.session.clone(),
        };
        let (compactor, _) = build_compaction(CompactionSetup {
            provider,
            tool_specs: tools.to_vec(),
            reasoning: self.config.reasoning,
            compaction: self.compaction.clone(),
            context_window: Some(context_window),
            usage_recording: ProviderRequestUsageRecording::default(),
            diagnostics: diagnostics.clone(),
            recall: Some(recall),
            todo: None,
        });
        let request = CompactionRequest::new(point.messages.clone(), CancellationToken::new())
            .with_trigger(CompactionTrigger::Automatic)
            .with_tool_specs(tools.to_vec())
            .with_context_estimate(ContextEstimate::from_estimated_tokens(before_tokens));
        let result = compactor.compact(request).await;
        Ok((
            result
                .map(rho_sdk::CompactionOutput::into_messages)
                .map_err(|error| error.to_string()),
            diagnostics.last_compaction(),
        ))
    }

    /// Asks one probe from the compacted history and scores the answer.
    /// Request failures become a scored-as-missing result with the error kept.
    async fn ask(
        &self,
        probe: Probe,
        reference: &Reference,
        compacted: &[Message],
        tools: &[ToolSpec],
    ) -> ProbeResult {
        let mut messages = compacted.to_vec();
        messages.push(Message::user_text(format!(
            "{ANSWER_SYSTEM_PROMPT}\n\nQuestion: {}",
            probe.question()
        )));
        let started = Instant::now();
        let answer = send(
            self.answerer.as_ref(),
            &messages,
            tools,
            self.config.reasoning,
        )
        .await;
        let answer_latency_ms = elapsed_ms(started);
        let (answer, answer_usage) = match answer {
            Ok(answer) => answer,
            Err(error) => {
                return ProbeResult::failed(probe, reference, error, answer_latency_ms);
            }
        };
        let (score, judge) = match (probe.scoring(), reference) {
            (Scoring::ExactMatch, Reference::Paths(paths)) => {
                (Some(probes::path_recall(paths, &answer)), None)
            }
            (Scoring::Judge, _) | (Scoring::ExactMatch, Reference::Facts(_)) => {
                let started = Instant::now();
                let graded = self.grade(probe, reference, &answer).await;
                let latency_ms = elapsed_ms(started);
                match graded {
                    Ok((score, verdict, usage)) => (
                        score,
                        Some(JudgeReport {
                            verdict,
                            usage,
                            latency_ms,
                        }),
                    ),
                    Err(error) => (
                        None,
                        Some(JudgeReport {
                            verdict: format!("judge failed: {error}"),
                            usage: ModelUsage::default(),
                            latency_ms,
                        }),
                    ),
                }
            }
        };
        ProbeResult {
            probe,
            question: probe.question(),
            reference: reference.text(),
            answer,
            score,
            answer_usage,
            answer_latency_ms,
            judge,
            error: None,
        }
    }

    async fn grade(
        &self,
        probe: Probe,
        reference: &Reference,
        answer: &str,
    ) -> anyhow::Result<(Option<f64>, String, ModelUsage)> {
        let messages = [
            Message::System(JUDGE_SYSTEM_PROMPT.into()),
            Message::user_text(format!(
                "Question:\n{}\n\nReference:\n{}\n\nAnswer:\n{answer}",
                probe.question(),
                reference.text()
            )),
        ];
        let (verdict, usage) =
            send(self.judge.as_ref(), &messages, &[], self.config.reasoning).await?;
        Ok((parse_score(&verdict), verdict, usage))
    }
}

/// The first `{"score": ...}` object in a judge reply, clamped to 0..=1.
fn parse_score(verdict: &str) -> Option<f64> {
    let start = verdict.find('{')?;
    let end = verdict.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(verdict.get(start..=end)?).ok()?;
    Some(value.get("score")?.as_f64()?.clamp(0.0, 1.0))
}

/// One text-only request, not recorded in the usage ledger.
async fn send(
    provider: &dyn ModelProvider,
    messages: &[Message],
    tools: &[ToolSpec],
    reasoning: rho_sdk::ReasoningLevel,
) -> anyhow::Result<(String, ModelUsage)> {
    let (ModelResponse::Assistant(blocks), usage) = crate::usage::send_recorded(
        provider,
        ModelRequest {
            messages,
            tools,
            cancellation: CancellationToken::new(),
            reasoning_level: reasoning,
            prompt_cache_key: None,
        },
        ProviderRequestUsageContext::for_purpose(provider.identity(), "compaction_eval"),
        ProviderRequestUsageRecording::default(),
    )
    .await?;
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<String>();
    Ok((text, usage))
}

/// Tool specs for every tool the history calls. Names only; replayed
/// requests never execute tools, and some providers reject history that
/// calls a tool the request does not declare.
fn replay_tool_specs(messages: &[Message]) -> Vec<ToolSpec> {
    let names = messages
        .iter()
        .filter_map(Message::completed_assistant_content)
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call.name.clone()),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    names
        .into_iter()
        .map(|name| ToolSpec {
            description: format!("{name} (replayed from a saved session; unavailable)"),
            name,
            input_schema: serde_json::json!({"type": "object"}),
        })
        .collect()
}

async fn build_reference(
    config: &Config,
    reference: &str,
) -> anyhow::Result<Arc<dyn ModelProvider>> {
    let (provider, model) = split_reference(reference)?;
    build(config, provider, model).await
}

async fn build(
    config: &Config,
    provider: &str,
    model: &str,
) -> anyhow::Result<Arc<dyn ModelProvider>> {
    let mut selected = config.clone();
    if selected.provider != provider {
        selected.auth = default_auth(config, provider);
    }
    selected.provider = provider.into();
    selected.model = model.into();
    let store = Arc::new(crate::credential_store::AppCredentialStore);
    crate::credential_store::build_provider_from_config_ensuring_catalog(&selected, store)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "could not build {}: {error}",
                rho_providers::provider::model_reference(provider, model)
            )
        })
}

/// Loads config with credentials and root CLI overrides applied, as the
/// offline evals need it.
pub(super) fn load_eval_config(cli: &Cli) -> anyhow::Result<Config> {
    let repository = ConfigRepository::new(cli.config.clone());
    let mut config = repository.load()?;
    config.providers.activate()?;
    let config_path = super::bootstrap::absolute_config_path(&repository)?;
    crate::credential_store::initialize_from_config(&mut config, &config_path)?;
    super::cli_config::apply_overrides_allowing_empty_cache(&mut config, cli)?;
    Ok(config)
}

pub(super) fn split_reference(reference: &str) -> anyhow::Result<(&str, &str)> {
    reference
        .split_once('/')
        .filter(|(provider, model)| !provider.is_empty() && !model.is_empty())
        .ok_or_else(|| anyhow::anyhow!("expected provider/model, got '{reference}'"))
}

/// The configured auth for the current provider, else the provider's first.
pub(super) fn default_auth(config: &Config, provider: &str) -> String {
    if provider == config.provider {
        return config.auth.clone();
    }
    rho_providers::provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.default_auth().id.to_string())
        .unwrap_or_else(|| config.auth.clone())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Hides a provider's native compaction so the text tiers are measured.
struct NoNativeCompaction(Arc<dyn ModelProvider>);

impl ModelProvider for NoNativeCompaction {
    fn cancellation_mode(&self) -> rho_sdk::provider::ProviderCancellationMode {
        self.0.cancellation_mode()
    }

    fn identity(&self) -> rho_sdk::model::ModelIdentity {
        self.0.identity()
    }

    fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> rho_sdk::provider::ProviderFuture<'a> {
        self.0.send_turn(request)
    }

    fn send_turn_stream_with_options<'a>(
        &'a self,
        request: ModelRequest<'a>,
        options: rho_sdk::provider::ModelRequestOptions,
        events: rho_sdk::provider::ProviderEventSender,
    ) -> rho_sdk::provider::ProviderFuture<'a> {
        self.0
            .send_turn_stream_with_options(request, options, events)
    }
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    probe_set_version: u32,
    configuration: Configuration,
    sessions: Vec<SessionReport>,
}

#[derive(Serialize)]
struct Configuration {
    session_model: String,
    reasoning: String,
    summarizer: Option<String>,
    tiers: &'static str,
    threshold_percent: u8,
    target_percent: u8,
    /// `None` sizes each point's window to put it at the threshold.
    context_window: Option<u64>,
    answer_model: String,
    judge_model: String,
}

#[derive(Serialize)]
struct SessionReport {
    session_id: String,
    points: Vec<PointReport>,
}

#[derive(Serialize)]
struct PointReport {
    node_id: String,
    context_window: u64,
    before_tokens: u64,
    after_tokens: Option<u64>,
    /// Text of the summary the compaction wrote, for reviewing its quality.
    /// `None` for native compaction, whose output is opaque, and for points
    /// that needed no summary.
    summary: Option<String>,
    /// The production compactor's own record: tier, request path, usage,
    /// and latency.
    compaction: Option<CompactionRecord>,
    error: Option<String>,
    probes: Vec<ProbeResult>,
}

#[derive(Serialize)]
struct ProbeResult {
    probe: Probe,
    question: &'static str,
    reference: String,
    answer: String,
    /// 0 to 1. `None` when the answer or the judge failed.
    score: Option<f64>,
    answer_usage: ModelUsage,
    answer_latency_ms: u64,
    judge: Option<JudgeReport>,
    error: Option<String>,
}

impl ProbeResult {
    fn failed(
        probe: Probe,
        reference: &Reference,
        error: anyhow::Error,
        answer_latency_ms: u64,
    ) -> Self {
        Self {
            probe,
            question: probe.question(),
            reference: reference.text(),
            answer: String::new(),
            score: None,
            answer_usage: ModelUsage::default(),
            answer_latency_ms,
            judge: None,
            error: Some(error.to_string()),
        }
    }
}

#[derive(Serialize)]
struct JudgeReport {
    verdict: String,
    usage: ModelUsage,
    latency_ms: u64,
}
