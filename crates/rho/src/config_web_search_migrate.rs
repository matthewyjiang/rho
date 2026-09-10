use super::{
    parse_search_endpoint_url, ExaSearchConnection, OpenAiSearchConnection, SearchBackend,
    WebSearchMode, WebSearchSettings,
};

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct WebSearchPartial {
    pub hosted: Option<bool>,
    pub provider: Option<String>,
    pub mode: Option<WebSearchMode>,
    pub backend: Option<SearchBackend>,
    pub openai: Option<OpenAiSearchPartial>,
    pub exa: Option<ExaSearchPartial>,
    pub brave: Option<EndpointPartial>,
    pub firecrawl: Option<EndpointPartial>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct OpenAiSearchPartial {
    pub connection: Option<OpenAiSearchConnection>,
    pub api_base_url: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct ExaSearchPartial {
    pub connection: Option<ExaSearchConnection>,
    pub api_base_url: Option<String>,
    pub mcp_url: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct EndpointPartial {
    pub api_base_url: Option<String>,
}

pub(in crate::config) enum WebSearchLoadWarning {
    Normalized {
        key: &'static str,
        from: String,
        to: String,
    },
    Migrated {
        key: &'static str,
        from: String,
        to: String,
    },
}

pub(in crate::config) fn resolve_web_search_settings(
    partial: WebSearchPartial,
    warnings: &mut Vec<WebSearchLoadWarning>,
) -> anyhow::Result<WebSearchSettings> {
    let mut settings = WebSearchSettings::default();
    apply_backend_partials(&mut settings, &partial)?;

    let has_legacy = partial.hosted.is_some() || partial.provider.is_some();
    if let Some(mode) = partial.mode {
        settings.mode = mode;
    } else if has_legacy {
        let (mode, backend, mut migrated) =
            migrate_legacy_web_search(partial.hosted, partial.provider.as_deref())?;
        settings.mode = mode;
        settings.backend = backend;
        warnings.append(&mut migrated);
    }
    if let Some(backend) = partial.backend {
        settings.backend = backend;
    }
    if has_legacy && partial.mode.is_none() && settings.mode != WebSearchMode::Off {
        warn_if_implicit_connection(&settings, &partial, warnings);
    }
    Ok(settings)
}

fn apply_backend_partials(
    settings: &mut WebSearchSettings,
    partial: &WebSearchPartial,
) -> anyhow::Result<()> {
    if let Some(openai) = &partial.openai {
        if let Some(connection) = openai.connection {
            settings.openai.connection = connection;
        }
        settings.openai.api_base_url = parse_optional_endpoint(
            "web_search.openai.api_base_url",
            openai.api_base_url.as_deref(),
        )?;
    }
    if let Some(exa) = &partial.exa {
        if let Some(connection) = exa.connection {
            settings.exa.connection = connection;
        }
        settings.exa.api_base_url =
            parse_optional_endpoint("web_search.exa.api_base_url", exa.api_base_url.as_deref())?;
        settings.exa.mcp_url =
            parse_optional_endpoint("web_search.exa.mcp_url", exa.mcp_url.as_deref())?;
    }
    if let Some(brave) = &partial.brave {
        settings.brave.api_base_url = parse_optional_endpoint(
            "web_search.brave.api_base_url",
            brave.api_base_url.as_deref(),
        )?;
    }
    if let Some(firecrawl) = &partial.firecrawl {
        settings.firecrawl.api_base_url = parse_optional_endpoint(
            "web_search.firecrawl.api_base_url",
            firecrawl.api_base_url.as_deref(),
        )?;
    }
    Ok(())
}

fn parse_optional_endpoint(field: &str, value: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    parse_search_endpoint_url(field, value)?;
    Ok(Some(value.to_string()))
}

fn warn_if_implicit_connection(
    settings: &WebSearchSettings,
    partial: &WebSearchPartial,
    warnings: &mut Vec<WebSearchLoadWarning>,
) {
    let openai_connection_set = partial
        .openai
        .as_ref()
        .and_then(|openai| openai.connection)
        .is_some();
    let exa_connection_set = partial
        .exa
        .as_ref()
        .and_then(|exa| exa.connection)
        .is_some();
    if matches!(settings.backend, SearchBackend::OpenAi) && !openai_connection_set {
        warnings.push(WebSearchLoadWarning::Migrated {
            key: "web_search.openai.connection",
            from: "implicit OpenAI transport".into(),
            to: format!(
                "{} (set web_search.openai.connection to \"api\" or \"codex\")",
                OpenAiSearchConnection::Api
            ),
        });
    }
    if matches!(settings.backend, SearchBackend::Exa) && !exa_connection_set {
        warnings.push(WebSearchLoadWarning::Migrated {
            key: "web_search.exa.connection",
            from: "implicit Exa transport".into(),
            to: format!(
                "{} (set web_search.exa.connection to \"api\" or \"mcp\")",
                ExaSearchConnection::Api
            ),
        });
    }
}

/// Map old `hosted` + `provider` onto mode/backend.
///
/// Off and a concrete backend are preserved. `hosted=true` with
/// `provider=disabled` used to mean native-only search; that cannot be
/// represented without silently enabling a client backend, so load fails until
/// `mode` is set. Old `provider = "auto"` cannot keep cross-service fallback.
pub(in crate::config) fn migrate_legacy_web_search(
    hosted: Option<bool>,
    provider: Option<&str>,
) -> anyhow::Result<(WebSearchMode, SearchBackend, Vec<WebSearchLoadWarning>)> {
    let hosted = hosted.unwrap_or(true);
    let normalized = provider.map(|value| value.trim().to_ascii_lowercase());
    let trimmed = normalized.as_deref();
    if hosted && trimmed == Some("disabled") {
        anyhow::bail!(
            "web_search hosted=true with provider=disabled used to enable native-only search; set web_search.mode to \"auto\", \"backend\", or \"off\""
        );
    }

    let mut warnings = Vec::new();
    if trimmed == Some("disabled") {
        return Ok((WebSearchMode::Off, SearchBackend::OpenAi, warnings));
    }

    let mode = if hosted {
        WebSearchMode::Auto
    } else {
        WebSearchMode::Backend
    };
    let backend = match trimmed {
        None | Some("") | Some("auto") => {
            warnings.push(WebSearchLoadWarning::Migrated {
                key: "web_search",
                from: format!("hosted={}, provider={}", hosted, provider.unwrap_or("auto")),
                to: format!(
                    "mode={mode}, backend=openai (Auto no longer tries Exa or Brave after a failure)"
                ),
            });
            SearchBackend::OpenAi
        }
        Some("openai") => SearchBackend::OpenAi,
        Some("exa") => SearchBackend::Exa,
        Some("brave") => SearchBackend::Brave,
        Some(other) => {
            warnings.push(WebSearchLoadWarning::Normalized {
                key: "web_search.provider",
                from: format!("\"{other}\""),
                to: format!("mode={}, backend={}", mode.as_str(), SearchBackend::OpenAi),
            });
            SearchBackend::OpenAi
        }
    };
    Ok((mode, backend, warnings))
}
