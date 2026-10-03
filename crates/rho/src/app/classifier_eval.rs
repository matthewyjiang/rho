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
    permission_classifier::{ClassifierModel, ClassifierVerdict, ClassifyRequest, ScreenOutcome},
};

#[path = "classifier_eval/cases.rs"]
mod cases;

use cases::{CaseSource, Decision, EvalCase};

/// Bump when the report shape changes.
const REPORT_SCHEMA_VERSION: u32 = 1;

pub(super) async fn run(args: &ClassifierEvalArgs, cli: &Cli) -> anyhow::Result<()> {
    let mut config = load_eval_config(cli)?;
    if let Some(reference) = &args.model {
        select_classifier_model(&mut config, reference)?;
    }
    let model = ClassifierModel::resolve(&config).await?;

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
        reasoning: model.reasoning().to_string(),
        cases: reports,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Points the classifier at `reference`, keeping any configured reasoning
/// override so a model comparison changes one thing at a time.
fn select_classifier_model(config: &mut Config, reference: &str) -> anyhow::Result<()> {
    let (provider, model) = split_reference(reference)?;
    let reasoning = config
        .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
        .and_then(|selection| selection.reasoning);
    let mut selection = InternalAgentModelConfig::new(
        provider.into(),
        model.into(),
        default_auth(config, provider),
    );
    selection.reasoning = reasoning;
    config.set_internal_agent_model_config(PERMISSION_CLASSIFIER_AGENT_ID, selection);
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
        screen: match trace.screen {
            ScreenOutcome::Skipped => "skipped",
            ScreenOutcome::Allowed => "allow",
            ScreenOutcome::Escalated => "escalate",
            ScreenOutcome::Failed => "failed",
        },
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
    reasoning: String,
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
    /// `None` when classification failed; production then denies.
    verdict: Option<Decision>,
    reason: Option<String>,
    error: Option<String>,
    latency_ms: u64,
}

#[cfg(test)]
#[path = "classifier_eval/cases_tests.rs"]
mod cases_tests;
