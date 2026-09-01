use pretty_assertions::assert_eq;
use rho_providers::{model::provider_models::ProviderModelHealth, provider};

use super::*;

// Covers: the disabled gate plans nothing; the live gate plans an endpoint
// probe only for the active provider or a user-configured host, then the
// binaries. Unused built-in defaults are not probed.
// Owner: pure unit
#[test]
fn plan_follows_gate_and_selected_or_configured_hosts() {
    let config = Config::default();

    assert!(plan_probes(&config, DoctorProbeGate::Disabled).is_empty());
    assert_eq!(
        plan_probes(&config, DoctorProbeGate::Live),
        vec![DoctorProbeId::Claude, DoctorProbeId::Rtk]
    );

    let mut active = Config::default();
    active.provider = "ollama".into();
    assert_endpoint_then_binaries(&active, "ollama");

    let mut configured = Config::default();
    configured
        .providers
        .set_endpoint("ollama", rho_providers::provider::OLLAMA_API_BASE)
        .unwrap();
    assert_endpoint_then_binaries(&configured, "ollama");
}

fn assert_endpoint_then_binaries(config: &Config, provider: &str) {
    let probes = plan_probes(config, DoctorProbeGate::Live);
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
    assert_eq!(failed[0].summary, "probe failed");
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
