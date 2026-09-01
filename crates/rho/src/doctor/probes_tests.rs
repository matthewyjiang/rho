use pretty_assertions::assert_eq;
use rho_providers::{model::provider_models::ProviderModelHealth, provider};

use super::*;

// Covers: the disabled gate plans nothing; the live gate plans an endpoint
// probe only for the active provider or a user-configured host, then the
// binaries. Unused built-in defaults are not probed. The active host is the
// argument, not `config.provider`, so an unsaved session provider is still
// planned.
// Owner: pure unit
#[test]
fn plan_follows_gate_and_selected_or_configured_hosts() {
    let config = Config::default();

    assert!(plan_probes(&config, &config.provider, DoctorProbeGate::Disabled).is_empty());
    assert_eq!(
        plan_probes(&config, &config.provider, DoctorProbeGate::Live),
        vec![DoctorProbeId::Claude, DoctorProbeId::Rtk]
    );

    let active = Config {
        provider: "ollama".into(),
        ..Default::default()
    };
    assert_endpoint_then_binaries(&active, &active.provider, "ollama");

    let mut configured = Config::default();
    configured
        .providers
        .set_endpoint("ollama", rho_providers::provider::OLLAMA_API_BASE)
        .unwrap();
    assert_endpoint_then_binaries(&configured, &configured.provider, "ollama");

    let unsaved = Config::default();
    assert_endpoint_then_binaries(&unsaved, "ollama", "ollama");
}

fn assert_endpoint_then_binaries(config: &Config, active_provider: &str, provider: &str) {
    let probes = plan_probes(config, active_provider, DoctorProbeGate::Live);
    let DoctorProbeId::ProviderEndpoint {
        provider: planned,
        endpoint,
    } = &probes[0]
    else {
        panic!("expected an endpoint probe first, got {:?}", probes[0]);
    };
    assert_eq!(planned, provider);
    assert!(
        provider::provider_descriptor(provider)
            .is_some_and(|descriptor| descriptor.probes_configured_endpoint()),
        "{provider} does not expose a model endpoint"
    );
    assert_eq!(
        config.resolved_provider_endpoint(provider).as_ref(),
        Some(endpoint)
    );
    assert_eq!(&probes[1..], &[DoctorProbeId::Claude, DoctorProbeId::Rtk]);
}

// Covers: placeholder rows, timed-out rows, and finished rows all cover the
// same identities so a probe never leaves a stale row behind.
// Owner: pure unit
#[test]
fn placeholders_and_results_cover_the_same_rows() {
    let ids = |checks: &[DoctorCheck]| {
        checks
            .iter()
            .map(|check| check.id.clone())
            .collect::<Vec<_>>()
    };
    let claude_ids = vec![DoctorCheckId::ClaudeAuth, DoctorCheckId::ClaudeBinary];

    let checking = placeholder_checks(&DoctorProbeId::Claude, ProbePlaceholder::Checking);
    assert_eq!(ids(&checking), claude_ids);
    assert!(checking
        .iter()
        .all(|check| check.status == DoctorStatus::Checking && check.summary == "checking"));

    let skipped = placeholder_checks(&DoctorProbeId::Claude, ProbePlaceholder::NotChecked);
    assert!(skipped
        .iter()
        .all(|check| check.status == DoctorStatus::Info && check.summary == "not checked"));

    let timed_out = probe_checks(
        &DoctorProbeOutcome::TimedOut(DoctorProbeId::Claude),
        "openai",
    );
    assert_eq!(ids(&timed_out), claude_ids);
    assert!(timed_out
        .iter()
        .all(|check| check.status == DoctorStatus::Warn && check.summary == "timed out"));

    let failed = probe_checks(&DoctorProbeOutcome::Failed(DoctorProbeId::Rtk), "openai");
    assert_eq!(ids(&failed), vec![DoctorCheckId::Rtk]);
    assert_eq!(
        (failed[0].status, failed[0].summary.as_str()),
        (DoctorStatus::Warn, "probe failed")
    );
}

fn ollama_endpoint_id() -> DoctorProbeId {
    DoctorProbeId::ProviderEndpoint {
        provider: "ollama".into(),
        endpoint: rho_providers::provider::OLLAMA_API_BASE.parse().unwrap(),
    }
}

// Covers: a hung or panicked active host must fail `rho doctor`; the same
// outcome on an unused configured host stays informational so CI still
// gates on the selected setup.
// Owner: pure unit
#[test]
fn timed_out_or_failed_active_endpoint_fails_ci() {
    let id = ollama_endpoint_id();
    for (name, outcome, summary) in [
        (
            "active timeout",
            DoctorProbeOutcome::TimedOut(id.clone()),
            "timed out",
        ),
        (
            "active failure",
            DoctorProbeOutcome::Failed(id.clone()),
            "probe failed",
        ),
    ] {
        let checks = probe_checks(&outcome, "ollama");
        assert_eq!(
            (checks[0].status, checks[0].summary.as_str()),
            (DoctorStatus::Fail, summary),
            "{name}"
        );
    }

    let unused = probe_checks(
        &DoctorProbeOutcome::TimedOut(ollama_endpoint_id()),
        "openai",
    );
    assert_eq!(unused[0].status, DoctorStatus::Info);
}

// Covers: finished outcomes become the rows their placeholders reserved.
// Owner: pure unit
#[test]
fn outcomes_map_to_rows() {
    let rtk = probe_checks(&DoctorProbeOutcome::Rtk { available: false }, "openai");
    assert_eq!(
        rtk,
        vec![
            DoctorCheck::new(DoctorCheckId::Rtk, "rtk", DoctorStatus::Info, "unavailable")
                .with_hint("optional shell-command rewriting helper")
        ]
    );

    let endpoint = probe_checks(
        &DoctorProbeOutcome::ProviderEndpoint {
            provider: "ollama".into(),
            health: ProviderModelHealth::ReachableWithModels { model_count: 1 },
        },
        "ollama",
    );
    assert_eq!(
        endpoint,
        vec![DoctorCheck::new(
            DoctorCheckId::ProviderEndpoint {
                provider: "ollama".into()
            },
            "Ollama connection",
            DoctorStatus::Ok,
            "reachable, 1 model",
        )]
    );
}

// Covers: a hung `rtk --version` child must be killed and reaped instead of
// pinning doctor past the 2s probe budget.
// Owner: OS or process
#[cfg(unix)]
#[tokio::test]
async fn hung_rtk_probe_is_killed_within_budget() {
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", "sleep 30"]);
    let started = std::time::Instant::now();
    assert!(!super::probe_rtk_command(command).await);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "hung probe must not exceed the 2s budget by much, elapsed {:?}",
        started.elapsed()
    );
}
