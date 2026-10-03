//! `rho __classifier_eval`: runs the production permission classifier over
//! eval cases and prints one JSON report. A local development tool driven by
//! `scripts/classifier_eval.py`; not part of CI.
//!
//! Labeled fixture cases measure false allows and false denies. Calls
//! replayed from saved sessions are unlabeled; comparing two reports on them
//! shows which decisions a classifier change flips. Case histories go only to
//! the classifier model, and eval requests stay out of the usage ledger.

use std::time::Instant;

use futures_util::future::join_all;
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

#[path = "classifier_eval/cases.rs"]
mod cases;

use cases::{CaseSource, Decision, EvalCase};

/// Bump when the report shape changes.
const REPORT_SCHEMA_VERSION: u32 = 2;

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
    let (model, permits) = (&model, &permits);
    let total = eval_cases.len();
    let reports = join_all(
        eval_cases
            .iter()
            .enumerate()
            .map(|(index, case)| async move {
                let _permit = permits.acquire().await;
                eprintln!("classifier eval: {}/{total} {}", index + 1, case.id);
                classify(model, case).await
            }),
    )
    .await;

    let identity = model.provider().identity();
    let report = Report {
        schema_version: REPORT_SCHEMA_VERSION,
        model: rho_providers::provider::model_reference(&identity.provider, &identity.model),
        auth,
        reasoning: model.reasoning().to_string(),
        screen_model: config
            .internal_agent_model(DECISION_SCREEN_ID)
            .map(InternalAgentModelConfig::display_reference),
        cases: reports,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
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
    let (verdict, reason, error) = match trace.result {
        Ok(ClassifierVerdict::Allow) => (Some(Decision::Allow), None, None),
        Ok(ClassifierVerdict::Deny { reason }) => (Some(Decision::Deny), Some(reason), None),
        Err(error) => (None, None, Some(format!("{error:#}"))),
    };
    CaseReport {
        id: case.id.clone(),
        source: case.source,
        category: case.category.clone(),
        label: case.label,
        pending: case.summary.clone(),
        input_digest: input_digest(case),
        screen: match &trace.screen {
            ScreenOutcome::Skipped => "skipped",
            ScreenOutcome::Allowed => "allow",
            ScreenOutcome::Escalated => "escalate",
            ScreenOutcome::Failed(_) => "failed",
        },
        screen_error: match trace.screen {
            ScreenOutcome::Failed(error) => Some(error),
            ScreenOutcome::Skipped | ScreenOutcome::Allowed | ScreenOutcome::Escalated => None,
        },
        screen_allow_probability: trace.screen_allow_probability,
        verdict,
        reason,
        error,
        latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// Fingerprint of what the classifier sees for a case, so a comparison can
/// tell a changed input from a changed decision without storing the history.
fn input_digest(case: &EvalCase) -> String {
    let input = serde_json::json!({
        "workspace": case.workspace,
        "history": case.history,
        "pending_call_id": case.pending_call_id,
        "pending": case.summary,
    });
    hex::encode(Sha256::digest(input.to_string().as_bytes()))
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    model: String,
    auth: String,
    reasoning: String,
    /// The model answering the screen; `None` when the classifier model does.
    screen_model: Option<String>,
    cases: Vec<CaseReport>,
}

#[derive(Serialize)]
struct CaseReport {
    id: String,
    source: CaseSource,
    category: Option<String>,
    label: Option<Decision>,
    pending: String,
    input_digest: String,
    screen: &'static str,
    /// Why the screen failed, when it did; the review then decided.
    screen_error: Option<String>,
    /// A decision-model screen's P(allow), from which its threshold is set.
    screen_allow_probability: Option<f64>,
    /// `None` when classification failed; production then denies.
    verdict: Option<Decision>,
    reason: Option<String>,
    error: Option<String>,
    latency_ms: u64,
}

#[cfg(test)]
#[path = "classifier_eval/cases_tests.rs"]
mod cases_tests;

#[cfg(test)]
#[path = "classifier_eval/select_tests.rs"]
mod select_tests;
