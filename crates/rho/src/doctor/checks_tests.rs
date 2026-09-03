use pretty_assertions::assert_eq;
use rho_providers::{
    credentials::{save_provider_api_key, MemoryCredentialStore},
    model::provider_models::ProviderModelHealth,
    provider::{self, ProviderAuthKind},
};

use super::*;
use crate::{
    claude_runtime::auth::ClaudeProbeSnapshot,
    cursor_runtime::auth::{CursorAuthError, CursorAuthStatus, CursorUserInfo},
    plugins::{PluginOrigin, PluginReportEntry, PluginScope, PluginStatus},
    tools::mcp::{
        report::{ConnectedServerReport, McpLiveServerState},
        McpServerReport, McpTransportSummary,
    },
};

fn auth_check<'a>(checks: &'a [DoctorCheck], auth_mode: &str) -> &'a DoctorCheck {
    checks
        .iter()
        .find(|check| {
            check.id
                == DoctorCheckId::ProviderAuth {
                    auth_mode: auth_mode.into(),
                }
        })
        .unwrap_or_else(|| panic!("no authentication row for {auth_mode}"))
}

// Covers: authentication rows come from the injected store and env-override
// hook only: a stored key is ok, a missing key warns only for the active auth
// mode, and every other missing or optional key stays informational.
// Owner: pure unit (no process env)
#[test]
fn authentication_rows_reflect_the_injected_store() {
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "openai", "sk-test").unwrap();

    let checks = authentication_checks(&store, "anthropic-api-key", &|_| false);

    assert_eq!(
        auth_check(&checks, "api-key"),
        &DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "api-key".into()
            },
            "OpenAI API key",
            DoctorStatus::Ok,
            "authenticated",
        )
    );
    assert_eq!(
        auth_check(&checks, "anthropic-api-key"),
        &DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "anthropic-api-key".into()
            },
            "Anthropic API key",
            DoctorStatus::Warn,
            "missing",
        )
        .with_hint("run /login anthropic-api-key")
    );

    let inactive = authentication_checks(&store, "api-key", &|_| false);
    assert_eq!(
        auth_check(&inactive, "anthropic-api-key"),
        &DoctorCheck::new(
            DoctorCheckId::ProviderAuth {
                auth_mode: "anthropic-api-key".into()
            },
            "Anthropic API key",
            DoctorStatus::Info,
            "missing",
        )
    );

    let optional_host = provider::providers()
        .into_iter()
        .find(|descriptor| descriptor.has_none_auth() && !descriptor.is_keyless())
        .expect("a provider that runs with or without a key");
    let optional_mode = optional_host
        .auth_modes()
        .find(|mode| mode.auth_kind != ProviderAuthKind::None)
        .expect("keyed auth mode");
    assert_eq!(
        auth_check(&checks, optional_mode.id).status,
        DoctorStatus::Info
    );

    let from_env = authentication_checks(&store, "api-key", &|mode| mode == "api-key");
    assert_eq!(
        auth_check(&from_env, "api-key").summary,
        "authenticated via environment"
    );
}

// Covers: every Herdr socket state maps to one status and summary.
// Owner: pure unit
#[test]
fn herdr_probe_maps_to_status() {
    let cases = [
        (
            HerdrProbe::NotConfigured,
            DoctorStatus::Info,
            "not configured",
        ),
        (HerdrProbe::Reachable, DoctorStatus::Ok, "connected"),
        (HerdrProbe::Unreachable, DoctorStatus::Fail, "unreachable"),
        (HerdrProbe::Unknown, DoctorStatus::Warn, "unknown"),
    ];
    for (probe, status, summary) in cases {
        let check = herdr_check(probe);
        assert_eq!(
            (check.status, check.summary.as_str()),
            (status, summary),
            "{probe:?}"
        );
    }
}

// Covers: the active host fails or warns; unused configured hosts stay
// informational so a down custom endpoint cannot fail `rho doctor`.
// Owner: pure unit
#[test]
fn endpoint_health_maps_to_status() {
    let cases = [
        (
            ProviderModelHealth::ReachableWithModels { model_count: 3 },
            "ollama",
            DoctorStatus::Ok,
            "reachable, 3 models",
            None,
        ),
        (
            ProviderModelHealth::ReachableWithoutModels,
            "ollama",
            DoctorStatus::Warn,
            "no models",
            Some("the endpoint is reachable but has no installed models"),
        ),
        (
            ProviderModelHealth::Unreachable {
                error: "connection refused".into(),
            },
            "ollama",
            DoctorStatus::Fail,
            "unreachable",
            Some("connection refused"),
        ),
        (
            ProviderModelHealth::InvalidResponse {
                error: "HTTP 500".into(),
            },
            "ollama",
            DoctorStatus::Fail,
            "invalid response",
            Some("HTTP 500"),
        ),
        (
            ProviderModelHealth::Unreachable {
                error: "connection refused".into(),
            },
            "openai",
            DoctorStatus::Info,
            "unreachable",
            Some("connection refused"),
        ),
        (
            ProviderModelHealth::InvalidResponse {
                error: "HTTP 500".into(),
            },
            "openai",
            DoctorStatus::Info,
            "invalid response",
            Some("HTTP 500"),
        ),
        (
            ProviderModelHealth::ReachableWithoutModels,
            "openai",
            DoctorStatus::Info,
            "no models",
            Some("the endpoint is reachable but has no installed models"),
        ),
    ];
    for (health, active_provider, status, summary, hint) in cases {
        let check = endpoint_check("ollama", &health, active_provider);
        assert_eq!(check.label, "Ollama connection");
        assert_eq!(
            (check.status, check.summary.as_str(), check.hint.as_deref()),
            (status, summary, hint),
            "{health:?} active={active_provider}"
        );
    }
}

// Covers: Claude rows distinguish signed in, signed out, and a probe error.
// Owner: pure unit
#[test]
fn claude_rows_cover_signed_in_signed_out_and_error() {
    let signed_in = ClaudeProbeSnapshot {
        auth: Ok(serde_json::from_value(
            serde_json::json!({ "loggedIn": true, "email": "dev@example.com", "subscriptionType": "max" }),
        )
        .unwrap()),
        version: Ok("2.1.0 (Claude Code)".into()),
    };
    let rows = claude_checks(&signed_in);
    assert_eq!(
        rows,
        vec![
            DoctorCheck::new(
                DoctorCheckId::ClaudeAuth,
                CLAUDE_AUTH_LABEL,
                DoctorStatus::Ok,
                "signed in as dev@example.com (max)",
            ),
            DoctorCheck::new(
                DoctorCheckId::ClaudeBinary,
                CLAUDE_BINARY_LABEL,
                DoctorStatus::Ok,
                "2.1.0 (Claude Code)",
            ),
        ]
    );

    let signed_out = ClaudeProbeSnapshot {
        auth: Ok(serde_json::from_value(serde_json::json!({ "loggedIn": false })).unwrap()),
        version: Err("claude code: binary not found on PATH".into()),
    };
    let rows = claude_checks(&signed_out);
    assert_eq!(
        (
            rows[0].status,
            rows[0].summary.as_str(),
            rows[0].hint.as_deref()
        ),
        (
            DoctorStatus::Warn,
            "not signed in",
            Some("run /login claude-code")
        )
    );
    assert_eq!(
        (
            rows[1].status,
            rows[1].summary.as_str(),
            rows[1].hint.as_deref()
        ),
        (
            DoctorStatus::Warn,
            "unavailable",
            Some("claude code: binary not found on PATH")
        )
    );
}

// Covers: Cursor doctor row is informational for missing binary / signed-out
// Owner: pure unit
#[test]
fn cursor_row_covers_signed_in_signed_out_and_not_installed() {
    let signed_in = cursor_check(
        &Ok(CursorAuthStatus {
            status: "authenticated".into(),
            is_authenticated: true,
            message: None,
            user_info: Some(CursorUserInfo {
                email: Some("dev@example.com".into()),
            }),
        }),
        Some("2026.08.25"),
    );
    let signed_out = cursor_check(
        &Ok(CursorAuthStatus {
            status: "unauthenticated".into(),
            is_authenticated: false,
            message: Some("Not logged in".into()),
            user_info: None,
        }),
        Some("2026.08.25"),
    );
    let missing = cursor_check(&Err(CursorAuthError::BinaryMissing), None);

    let cases = [
        (
            signed_in,
            DoctorStatus::Ok,
            "2026.08.25 signed in as dev@example.com",
        ),
        (
            signed_out,
            DoctorStatus::Info,
            "not signed in (run /login cursor)",
        ),
        (missing, DoctorStatus::Info, "not installed"),
    ];
    for (check, status, summary) in cases {
        assert_eq!(check.id, DoctorCheckId::Cursor);
        assert_eq!(check.label, CURSOR_LABEL);
        assert_eq!((check.status, check.summary.as_str()), (status, summary));
    }
}

// Covers: the MCP row follows the session summary: unconfigured is neutral,
// connected servers are healthy, a failed server degrades the row.
// Owner: pure unit
#[test]
fn mcp_row_follows_session_summary() {
    let unconfigured = mcp_check(&McpSessionReport::default());
    assert_eq!(
        (unconfigured.status, unconfigured.summary.as_str()),
        (DoctorStatus::Info, "not configured")
    );

    let connected = McpSessionReport {
        mode: McpLoadMode::Native,
        servers: vec![McpServerReport::connected(ConnectedServerReport {
            identity: "filesystem".into(),
            transport: McpTransportSummary::StreamableHttp {
                url: "https://example.com/mcp".into(),
            },
            tools: Vec::new(),
            instructions: None,
            live: McpLiveServerState::default(),
            filtered_out_count: 0,
            collision_skipped_count: 0,
        })],
    };
    let check = mcp_check(&connected);
    assert_eq!(
        (check.status, check.summary.as_str(), check.hint.as_deref()),
        (
            DoctorStatus::Ok,
            "connected",
            Some("1 connected server, 0 exported tools")
        )
    );

    let degraded = McpSessionReport {
        mode: McpLoadMode::Native,
        servers: vec![McpServerReport::failed(
            "filesystem",
            McpTransportSummary::StreamableHttp {
                url: "https://example.com/mcp".into(),
            },
            "connection refused",
        )],
    };
    let check = mcp_check(&degraded);
    assert_eq!(
        (check.status, check.summary.as_str(), check.hint.as_deref()),
        (
            DoctorStatus::Warn,
            "degraded",
            Some("1 server problem, 0 connected, 0 tools; run /mcp for details")
        )
    );
}

fn plugin(name: &str, status: PluginStatus) -> PluginReportEntry {
    PluginReportEntry {
        name: name.into(),
        version: None,
        description: None,
        root: format!("/plugins/{name}"),
        scope: PluginScope::User,
        origin: PluginOrigin::Install,
        enabled: status != PluginStatus::Disabled,
        status,
        problems: Vec::new(),
        skill_count: 1,
        mcp_server_count: 0,
        skill_names: vec!["hello".into()],
        mcp_server_names: Vec::new(),
    }
}

// Covers: the plugin row is neutral with no packages, healthy when every
// package loaded cleanly, and warns on a rejected package.
// Owner: pure unit
#[test]
fn plugins_row_flags_rejected_packages() {
    let none = plugins_check(&PluginLoadReport::default());
    assert_eq!(
        (none.status, none.summary.as_str()),
        (DoctorStatus::Info, "none discovered")
    );

    let clean = plugins_check(&PluginLoadReport {
        plugins: vec![plugin("hello", PluginStatus::Loaded)],
    });
    assert_eq!(
        (clean.status, clean.summary.as_str()),
        (DoctorStatus::Ok, "1 loaded")
    );

    let rejected = plugins_check(&PluginLoadReport {
        plugins: vec![
            plugin("hello", PluginStatus::Loaded),
            plugin("broken", PluginStatus::Rejected),
        ],
    });
    assert_eq!(
        (rejected.status, rejected.summary.as_str()),
        (DoctorStatus::Warn, "1 loaded, 1 rejected")
    );
}

// Covers: path rows verify writability on disk and always carry the path.
// Owner: filesystem unit
#[test]
fn path_check_reports_writability() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let sessions = dir.path().join("sessions");

    let writable = path_check(
        DoctorCheckId::ConfigPath,
        "Configuration",
        &config,
        PathKind::File,
    );
    assert_eq!(
        (
            writable.status,
            writable.summary.as_str(),
            writable.hint.as_deref()
        ),
        (
            DoctorStatus::Ok,
            "writable",
            Some(config.display().to_string().as_str())
        )
    );

    // A file where a directory is expected is not writable as a directory.
    std::fs::write(&sessions, b"not a dir").unwrap();
    let not_dir = path_check(
        DoctorCheckId::SessionRoot,
        "Sessions",
        &sessions,
        PathKind::Directory,
    );
    assert_eq!(
        (not_dir.status, not_dir.summary.as_str()),
        (DoctorStatus::Fail, "not writable")
    );
}
