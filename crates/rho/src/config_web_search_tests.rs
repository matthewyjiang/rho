use pretty_assertions::assert_eq;
use url::Url;

use super::super::{Config, ConfigWarning};
use super::{
    join_api_path, migrate_legacy_web_search, parse_search_endpoint_url,
    resolve_web_search_settings, resolved_endpoint_url, web_search_route, ExaSearchPartial,
    OpenAiSearchPartial, PartialWebSearchConfig, SearchBackend, WebSearchMode, WebSearchRoute,
    WebSearchSettings, FIRECRAWL_API_DEFAULT_BASE, OPENAI_API_DEFAULT_BASE,
};

// Covers: old off and concrete backends keep their intent; auto cannot keep
// cross-service fallback; native-only (hosted + disabled) is a load error.
// Owner: config migration
#[test]
fn legacy_web_search_migration_table() {
    let ok_cases = [
        (
            Some(false),
            Some("disabled"),
            WebSearchMode::Off,
            SearchBackend::OpenAi,
            false,
        ),
        (
            Some(false),
            Some("brave"),
            WebSearchMode::Backend,
            SearchBackend::Brave,
            false,
        ),
        (
            Some(true),
            Some("exa"),
            WebSearchMode::Auto,
            SearchBackend::Exa,
            false,
        ),
        (
            Some(false),
            Some("openai"),
            WebSearchMode::Backend,
            SearchBackend::OpenAi,
            false,
        ),
        (
            Some(true),
            Some("auto"),
            WebSearchMode::Auto,
            SearchBackend::OpenAi,
            true,
        ),
        (
            Some(false),
            Some("auto"),
            WebSearchMode::Backend,
            SearchBackend::OpenAi,
            true,
        ),
        (
            Some(true),
            Some("tavily"),
            WebSearchMode::Auto,
            SearchBackend::OpenAi,
            true,
        ),
    ];

    for (hosted, provider, mode, backend, warns) in ok_cases {
        let (got_mode, got_backend, warning) =
            migrate_legacy_web_search(hosted, provider).expect("legacy mapping");
        assert_eq!(
            (got_mode, got_backend),
            (mode, backend),
            "hosted={hosted:?} provider={provider:?}"
        );
        assert_eq!(
            !warning.is_empty(),
            warns,
            "hosted={hosted:?} provider={provider:?}"
        );
    }

    assert!(migrate_legacy_web_search(Some(true), Some("disabled")).is_err());
}

// Covers: backend and endpoint edits cannot enable legacy-disabled search.
// Owner: config migration
#[test]
fn web_search_partial_routing_preserves_legacy_intent() {
    for provider in ["disabled", "Disabled", " DISABLED "] {
        for backend in [None, Some(SearchBackend::Firecrawl)] {
            for hosted in [false, true] {
                let mut warnings = Vec::new();
                let result = resolve_web_search_settings(
                    PartialWebSearchConfig {
                        hosted: Some(hosted),
                        provider: Some(provider.into()),
                        backend,
                        ..PartialWebSearchConfig::default()
                    },
                    &mut warnings,
                );
                if hosted {
                    assert!(result.is_err(), "native-only needs an explicit mode");
                } else {
                    let settings = result.unwrap();
                    assert_eq!(
                        (settings.mode, settings.backend),
                        (
                            WebSearchMode::Off,
                            if backend.is_some() {
                                SearchBackend::Firecrawl
                            } else {
                                SearchBackend::OpenAi
                            }
                        )
                    );
                    assert!(warnings.is_empty());
                }
            }
        }
    }
    for partial in [
        PartialWebSearchConfig::default(),
        PartialWebSearchConfig {
            backend: Some(SearchBackend::Firecrawl),
            ..PartialWebSearchConfig::default()
        },
    ] {
        let mut warnings = Vec::new();
        let settings = resolve_web_search_settings(partial, &mut warnings).unwrap();
        assert_eq!(settings.mode, WebSearchMode::Auto);
        assert!(warnings.is_empty());
    }
}

// Covers: implicit OpenAI/Exa transport is a warning, not a silent Codex/API pick.
// Owner: config migration
#[test]
fn legacy_openai_exa_transport_warns_until_connection_is_set() {
    let mut warnings = Vec::new();
    let settings = resolve_web_search_settings(
        PartialWebSearchConfig {
            hosted: Some(false),
            provider: Some("openai".into()),
            ..PartialWebSearchConfig::default()
        },
        &mut warnings,
    )
    .unwrap();
    assert_eq!(settings.mode, WebSearchMode::Backend);
    assert_eq!(settings.backend, SearchBackend::OpenAi);
    assert!(warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::Migrated {
            key: "web_search.openai.connection",
            ..
        }
    )));

    warnings.clear();
    let settings = resolve_web_search_settings(
        PartialWebSearchConfig {
            hosted: Some(true),
            provider: Some("exa".into()),
            openai: None,
            exa: Some(ExaSearchPartial {
                connection: Some(super::ExaSearchConnection::Mcp),
                ..ExaSearchPartial::default()
            }),
            ..PartialWebSearchConfig::default()
        },
        &mut warnings,
    )
    .unwrap();
    assert_eq!(settings.exa.connection, super::ExaSearchConnection::Mcp);
    assert!(!warnings.iter().any(|warning| matches!(
        warning,
        ConfigWarning::Migrated {
            key: "web_search.exa.connection",
            ..
        }
    )));

    warnings.clear();
    let settings = resolve_web_search_settings(
        PartialWebSearchConfig {
            mode: Some(WebSearchMode::Backend),
            backend: Some(SearchBackend::OpenAi),
            openai: Some(OpenAiSearchPartial {
                connection: Some(super::OpenAiSearchConnection::Codex),
                api_base_url: None,
            }),
            ..PartialWebSearchConfig::default()
        },
        &mut warnings,
    )
    .unwrap();
    assert_eq!(
        settings.openai.connection,
        super::OpenAiSearchConnection::Codex
    );
    assert!(warnings.is_empty());
}

// Covers: Auto uses native only when runtime reports capability; Backend ignores it.
// Owner: web search routing
#[test]
fn web_search_route_table() {
    let auto = WebSearchSettings {
        mode: WebSearchMode::Auto,
        backend: SearchBackend::Brave,
        ..WebSearchSettings::default()
    };
    assert_eq!(
        web_search_route(&auto, /*hosted_supported*/ true),
        WebSearchRoute::Native
    );
    assert_eq!(
        web_search_route(&auto, /*hosted_supported*/ false),
        WebSearchRoute::Backend(SearchBackend::Brave)
    );

    let mut forced = auto.clone();
    forced.mode = WebSearchMode::Backend;
    assert_eq!(
        web_search_route(&forced, /*hosted_supported*/ true),
        WebSearchRoute::Backend(SearchBackend::Brave)
    );

    let mut off = auto;
    off.mode = WebSearchMode::Off;
    assert_eq!(
        web_search_route(&off, /*hosted_supported*/ true),
        WebSearchRoute::Off
    );
}

// Covers: search endpoints reject credentials/query/fragment, keep proxy prefixes,
// and treat cloud Firecrawl by origin+port, not string equality.
// Owner: web search endpoint security
#[test]
fn search_endpoint_validation_and_prefix_join() {
    parse_search_endpoint_url("field", "https://user:pass@example.com").unwrap_err();
    parse_search_endpoint_url("field", "https://example.com/search?q=1").unwrap_err();
    parse_search_endpoint_url("field", "https://example.com/search#frag").unwrap_err();
    parse_search_endpoint_url("field", "file:///etc/passwd").unwrap_err();
    parse_search_endpoint_url("field", "http://192.168.1.10:3002/firecrawl").unwrap();

    let base = Url::parse("http://home.example/firecrawl").unwrap();
    assert_eq!(
        join_api_path(&base, "v2/search").unwrap().as_str(),
        "http://home.example/firecrawl/v2/search"
    );
    assert_eq!(
        resolved_endpoint_url(None, OPENAI_API_DEFAULT_BASE, "responses")
            .unwrap()
            .as_str(),
        "https://api.openai.com/v1/responses"
    );
    let default = WebSearchSettings::default();
    assert!(default
        .destination(SearchBackend::Firecrawl)
        .is_default_origin());
    for configured in [
        FIRECRAWL_API_DEFAULT_BASE,
        "https://api.firecrawl.dev/",
        "https://api.firecrawl.dev:443",
    ] {
        let mut settings = WebSearchSettings::default();
        settings.firecrawl.api_base_url = Some(configured.into());
        assert!(
            settings
                .destination(SearchBackend::Firecrawl)
                .is_default_origin(),
            "{configured}"
        );
    }
    for configured in ["http://127.0.0.1:3002", "https://api.firecrawl.dev:8443"] {
        let mut settings = WebSearchSettings::default();
        settings.firecrawl.api_base_url = Some(configured.into());
        assert!(
            !settings
                .destination(SearchBackend::Firecrawl)
                .is_default_origin(),
            "{configured}"
        );
    }
}

#[test]
fn grouped_web_search_load_table() {
    let cases = [
        (
            r#"
[web_search]
hosted = false
provider = "brave"
"#,
            WebSearchMode::Backend,
            SearchBackend::Brave,
        ),
        (
            r#"
[web_search]
provider = "exa"
"#,
            WebSearchMode::Auto,
            SearchBackend::Exa,
        ),
        (
            r#"
[web_search]
hosted = false
provider = "disabled"
"#,
            WebSearchMode::Off,
            SearchBackend::OpenAi,
        ),
        (
            r#"
[web_search]
mode = "backend"
backend = "firecrawl"
"#,
            WebSearchMode::Backend,
            SearchBackend::Firecrawl,
        ),
    ];

    for (toml, mode, backend) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml).unwrap();
        let config = Config::load(Some(path)).unwrap();
        assert_eq!(
            (config.web_search.mode, config.web_search.backend),
            (mode, backend),
            "{toml}"
        );
    }
}

#[test]
fn grouped_web_search_native_only_legacy_is_a_load_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[web_search]
hosted = true
provider = "disabled"
"#,
    )
    .unwrap();

    assert!(Config::load(Some(path)).is_err());
}

#[test]
fn web_search_round_trips_mode_backend_endpoints_and_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = Config {
        rtk: false,
        web_search: WebSearchSettings {
            mode: WebSearchMode::Backend,
            backend: SearchBackend::Exa,
            ..WebSearchSettings::default()
        },
        ..Config::default()
    };
    config.web_search.exa.connection = super::ExaSearchConnection::Mcp;
    config.web_search.exa.api_base_url = Some("https://exa.example/v1".into());
    config.web_search.exa.mcp_url = Some("https://mcp.exa.example/mcp".into());
    config.web_search.openai.connection = super::OpenAiSearchConnection::Codex;
    config.web_search.firecrawl.api_base_url = Some("http://127.0.0.1:3002/firecrawl".into());

    config
        .save_with_store(
            path.clone(),
            &rho_providers::credentials::MemoryCredentialStore::default(),
        )
        .unwrap();
    let loaded = Config::load(Some(path)).unwrap();
    assert_eq!(loaded.web_search, config.web_search);
}
