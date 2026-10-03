//! `rho __classifier_eval`: runs the production permission classifier over
//! eval cases and prints one JSON report. A local development tool driven by
//! `scripts/classifier_eval.py`; not part of CI.
//!
//! Labeled fixture cases measure false allows and false denies. Calls
//! replayed from saved sessions are unlabeled; comparing two reports on them
//! shows which decisions a classifier change flips. Case histories go only to
//! the classifier model, and eval requests stay out of the usage ledger.
//!
//! `--batched` evaluates requests the agent made at once instead; see
//! [`batched`].

use std::{path::Path, time::Instant};

use futures_util::future::join_all;
use rho_providers::model::Message;
use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::compaction_eval::{default_auth, load_eval_config, split_reference};
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    cli::{ClassifierEvalArgs, Cli},
    config::{Config, InternalAgentModelConfig},
    permission_classifier::{
        ClassifierModel, ClassifierVerdict, ClassifyRequest, ScreenOutcome, DECISION_SCREEN_ID,
    },
};

#[path = "classifier_eval/batch_cases.rs"]
mod batch_cases;
#[path = "classifier_eval/batched.rs"]
mod batched;
#[path = "classifier_eval/cases.rs"]
mod cases;

use batched::ReviewScope;
use cases::{CaseSource, Decision, EvalCase};

/// Bump when the single-request report shape changes.
const REPORT_SCHEMA_VERSION: u32 = 2;
/// Bump when the batched report shape changes.
const BATCHED_SCHEMA_VERSION: u32 = 3;

pub(super) async fn run(args: &ClassifierEvalArgs, cli: &Cli) -> anyhow::Result<()> {
    let mut config = load_eval_config(cli)?;
    if let Some(reference) = &args.model {
        select_model(&mut config, PERMISSION_CLASSIFIER_AGENT_ID, reference)?;
    }
    if let Some(reference) = &args.screen_model {
        select_model(&mut config, DECISION_SCREEN_ID, reference)?;
    }
    let model = ClassifierModel::resolve(&config).await?;
    let auth = config
        .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
        .and_then(InternalAgentModelConfig::rho)
        .map(|selection| selection.auth.clone())
        .unwrap_or_default();

    let identity = model.provider().identity();
    let header = ReportHeader {
        model: rho_providers::provider::model_reference(&identity.provider, &identity.model),
        auth,
        reasoning: model.reasoning().to_string(),
        screen_model: config
            .internal_agent_model(DECISION_SCREEN_ID)
            .map(InternalAgentModelConfig::display_reference),
    };
    let report = if args.batched {
        let scope = match args.review_all {
            true => ReviewScope::All,
            false => ReviewScope::Escalated,
        };
        serde_json::to_string_pretty(&Report {
            schema_version: BATCHED_SCHEMA_VERSION,
            mode: Some("batched"),
            header,
            cases: run_batches(args, &model, scope).await?,
        })?
    } else {
        serde_json::to_string_pretty(&Report {
            schema_version: REPORT_SCHEMA_VERSION,
            mode: None,
            header,
            cases: run_cases(args, &model).await?,
        })?
    };
    println!("{report}");
    Ok(())
}

async fn run_cases(
    args: &ClassifierEvalArgs,
    model: &ClassifierModel,
) -> anyhow::Result<Vec<CaseReport>> {
    let mut eval_cases = Vec::new();
    for path in &args.cases {
        eval_cases.extend(cases::load_fixture_file(path)?);
    }
    for path in &args.sessions {
        eval_cases.extend(cases::replay_session(path, args.per_session.get())?);
    }
    anyhow::ensure!(
        !eval_cases.is_empty(),
        "no eval cases: pass --cases or --session"
    );

    // `--jobs` caps classifications in flight. Concurrent requests queue at
    // the provider and inflate per-case latency, so the default is one.
    let permits = tokio::sync::Semaphore::new(args.jobs.get());
    let permits = &permits;
    let total = eval_cases.len();
    Ok(join_all(
        eval_cases
            .iter()
            .enumerate()
            .map(|(index, case)| async move {
                let _permit = permits.acquire().await;
                eprintln!("classifier eval: {}/{total} {}", index + 1, case.id);
                classify(model, case).await
            }),
    )
    .await)
}

/// [`run_cases`] for `--batched`: `--jobs` caps batches in flight.
async fn run_batches(
    args: &ClassifierEvalArgs,
    model: &ClassifierModel,
    scope: ReviewScope,
) -> anyhow::Result<Vec<batched::MemberReport>> {
    let mut batches = Vec::new();
    for path in &args.batches {
        batches.extend(batch_cases::load_batch_file(path)?);
    }
    for path in &args.sessions {
        batches.extend(batch_cases::replay_session(path, args.per_session.get())?);
    }
    anyhow::ensure!(
        !batches.is_empty(),
        "no eval batches: pass --batches or a --session with parallel calls"
    );

    let permits = tokio::sync::Semaphore::new(args.jobs.get());
    let permits = &permits;
    let total = batches.len();
    let reports = join_all(batches.iter().enumerate().map(|(index, batch)| async move {
        let _permit = permits.acquire().await;
        eprintln!("classifier eval: batch {}/{total} {}", index + 1, batch.id);
        batched::classify_batch(model, batch, scope).await
    }))
    .await;
    Ok(reports.into_iter().flatten().collect())
}

/// Points the classifier at `reference`, keeping any configured reasoning
/// override, and the configured auth when the provider stays the same, so a
/// model comparison changes one thing at a time.
/// Points the `entry` config table at `reference`, keeping the entry's
/// reasoning and screen allow threshold, and its auth when the provider stays
/// the same.
fn select_model(config: &mut Config, entry: &str, reference: &str) -> anyhow::Result<()> {
    let (provider, model) = split_reference(reference)?;
    let current = config.internal_agent_model(entry);
    let reasoning = current.and_then(|selection| selection.reasoning);
    let auth = current
        .and_then(InternalAgentModelConfig::rho)
        .filter(|selection| selection.provider == provider)
        .map(|selection| selection.auth.clone())
        .unwrap_or_else(|| default_auth(config, provider));
    let allow_threshold_percent = current
        .and_then(InternalAgentModelConfig::rho)
        .and_then(|selection| selection.allow_threshold_percent);
    let mut selection = InternalAgentModelConfig::new(provider.into(), model.into(), auth);
    selection.reasoning = reasoning;
    if let crate::config::InternalAgentTarget::Rho(rho) = &mut selection.target {
        rho.allow_threshold_percent = allow_threshold_percent;
    }
    config.set_internal_agent_model_config(entry, selection);
    Ok(())
}

async fn classify(model: &ClassifierModel, case: &EvalCase) -> CaseReport {
    let started = Instant::now();
    let session_id = SessionId::new();
    let request = ClassifyRequest {
        history: &case.history,
        pending: &case.pending,
        cancellation: CancellationToken::new(),
        session_id: &session_id,
        workspace_path: &case.workspace,
        usage_recording: ProviderRequestUsageRecording::default(),
    };
    let trace = model
        .classify_with_pending_call(request, &case.pending_call_id)
        .await;
    CaseReport {
        id: case.id.clone(),
        source: case.source,
        category: case.category.clone(),
        label: case.label,
        pending: case.summary.clone(),
        input_digest: input_digest(
            &case.workspace,
            &case.history,
            &case.pending_call_id,
            &case.summary,
        ),
        screen: ScreenReport::of(trace.screen, trace.screen_allow_probability),
        outcome: Outcome::of(trace.result, elapsed_ms(started)),
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Fingerprint of what the classifier sees for a request, so a comparison
/// can tell a changed input from a changed decision without storing the
/// history.
fn input_digest(workspace: &Path, history: &[Message], call_id: &str, summary: &str) -> String {
    let input = serde_json::json!({
        "workspace": workspace,
        "history": history,
        "pending_call_id": call_id,
        "pending": summary,
    });
    hex::encode(Sha256::digest(input.to_string().as_bytes()))
}

#[derive(Serialize)]
struct Report<C> {
    schema_version: u32,
    /// `batched` for a `--batched` report; absent for single requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'static str>,
    #[serde(flatten)]
    header: ReportHeader,
    cases: Vec<C>,
}

#[derive(Serialize)]
struct ReportHeader {
    model: String,
    auth: String,
    reasoning: String,
    /// The model answering the screen; `None` when the classifier model does.
    screen_model: Option<String>,
}

#[derive(Serialize)]
struct CaseReport {
    id: String,
    source: CaseSource,
    category: Option<String>,
    label: Option<Decision>,
    pending: String,
    input_digest: String,
    #[serde(flatten)]
    screen: ScreenReport,
    #[serde(flatten)]
    outcome: Outcome,
}

/// What stage 1 did.
#[derive(Serialize)]
struct ScreenReport {
    screen: &'static str,
    /// Why the screen failed, when it did; the review then decided.
    screen_error: Option<String>,
    /// A decision-model screen's P(allow), from which its threshold is set.
    screen_allow_probability: Option<f64>,
}

impl ScreenReport {
    fn of(screen: ScreenOutcome, allow_probability: Option<f64>) -> Self {
        let (name, error) = match screen {
            ScreenOutcome::Skipped => ("skipped", None),
            ScreenOutcome::Allowed => ("allow", None),
            ScreenOutcome::Escalated => ("escalate", None),
            ScreenOutcome::Failed(error) => ("failed", Some(error)),
        };
        Self {
            screen: name,
            screen_error: error,
            screen_allow_probability: allow_probability,
        }
    }
}

/// A classification's result and how long it took.
#[derive(Clone, Serialize)]
struct Outcome {
    /// `None` when classification failed; production then denies.
    verdict: Option<Decision>,
    reason: Option<String>,
    error: Option<String>,
    latency_ms: u64,
}

impl Outcome {
    fn of(result: anyhow::Result<ClassifierVerdict>, latency_ms: u64) -> Self {
        let (verdict, reason, error) = match result {
            Ok(ClassifierVerdict::Allow) => (Some(Decision::Allow), None, None),
            Ok(ClassifierVerdict::Deny { reason }) => (Some(Decision::Deny), Some(reason), None),
            Err(error) => (None, None, Some(format!("{error:#}"))),
        };
        Self {
            verdict,
            reason,
            error,
            latency_ms,
        }
    }
}

#[cfg(test)]
#[path = "classifier_eval/cases_tests.rs"]
mod cases_tests;

#[cfg(test)]
#[path = "classifier_eval/batch_cases_tests.rs"]
mod batch_cases_tests;

#[cfg(test)]
#[path = "classifier_eval/select_tests.rs"]
mod select_tests;
