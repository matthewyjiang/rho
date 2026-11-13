use pretty_assertions::assert_eq;
use url::Url;

use super::super::Config;
use super::{
    join_api_path, parse_search_endpoint_url, resolved_endpoint_url, web_search_route,
    SearchBackend, WebSearchMode, WebSearchRoute, WebSearchSettings, FIRECRAWL_API_DEFAULT_BASE,
    OPENAI_API_DEFAULT_BASE,
};

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

// Covers: every web-search setting survives a save and load.
// Owner: config registry
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

    config.save(Some(path.clone())).unwrap();
    let loaded = Config::load(Some(path)).unwrap();
    assert_eq!(loaded.web_search, config.web_search);
}
