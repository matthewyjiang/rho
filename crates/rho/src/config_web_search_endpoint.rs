use url::Url;

pub const OPENAI_API_DEFAULT_BASE: &str = "https://api.openai.com/v1";
pub const OPENAI_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const EXA_API_DEFAULT_BASE: &str = "https://api.exa.ai";
pub const EXA_MCP_DEFAULT_URL: &str = "https://mcp.exa.ai/mcp";
pub const BRAVE_API_DEFAULT_BASE: &str = "https://api.search.brave.com";
pub const FIRECRAWL_API_DEFAULT_BASE: &str = "https://api.firecrawl.dev";

pub fn parse_search_endpoint_url(field: &str, value: &str) -> anyhow::Result<Url> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{field} is empty");
    }
    let parsed =
        Url::parse(trimmed).map_err(|error| anyhow::anyhow!("invalid {field}: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("{field} must use http or https");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("{field} must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        anyhow::bail!("{field} must not contain a query or fragment");
    }
    Ok(parsed)
}

/// Join `relative` onto `base` without dropping reverse-proxy path prefixes.
pub fn join_api_path(base: &Url, relative: &str) -> anyhow::Result<Url> {
    let relative = relative.trim().trim_start_matches('/');
    if relative.is_empty() {
        return Ok(base.clone());
    }
    let mut base = base.clone();
    if !base.path().ends_with('/') {
        let mut path = base.path().to_string();
        path.push('/');
        base.set_path(&path);
    }
    base.join(relative)
        .map_err(|error| anyhow::anyhow!("invalid search endpoint path: {error}"))
}

pub fn resolved_endpoint_url(
    configured: Option<&str>,
    default_base: &str,
    relative: &str,
) -> anyhow::Result<Url> {
    let base = match configured {
        Some(value) => parse_search_endpoint_url("web search endpoint", value)?,
        None => Url::parse(default_base).expect("default search endpoint is valid"),
    };
    join_api_path(&base, relative)
}

pub fn firecrawl_uses_cloud_default(configured: Option<&str>) -> bool {
    let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let Ok(parsed) = parse_search_endpoint_url("web_search.firecrawl.api_base_url", configured)
    else {
        return false;
    };
    let Ok(default) = Url::parse(FIRECRAWL_API_DEFAULT_BASE) else {
        return false;
    };
    origin_and_prefix(&parsed) == origin_and_prefix(&default)
}

/// Scheme, host, explicit port, and path prefix. Default ports stay implicit so
/// `https://host` and `https://host:443` compare equal.
fn origin_and_prefix(url: &Url) -> String {
    let path = url.path().trim_end_matches('/');
    match url.port() {
        Some(port) => format!(
            "{}://{}:{}{path}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            port
        ),
        None => format!(
            "{}://{}{path}",
            url.scheme(),
            url.host_str().unwrap_or_default()
        ),
    }
}
