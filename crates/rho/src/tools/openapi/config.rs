//! `[openapi.<source>]` configuration and the per-session source plan.
//!
//! Config is the trust boundary, as for MCP: declaring a source is permission
//! to fetch its spec at session start and expose its read operations. Sources
//! come from the user config and from the project file
//! `<project root>/.rho/openapi.toml`. A repository may declare a source, but it
//! cannot attach credentials: `auth` and `headers_from_env` are rejected there,
//! and a user source with the same name replaces the project one entirely, so
//! a repository can never point a user's credentials at its own server.
//!
//! The raw tables round-trip through config saves unchanged. Semantic checks
//! run when a session plans its sources, so one bad source fails alone.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use http::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;

use super::filter::OperationFilter;
use crate::{
    tools::tool_name::sanitize_segment,
    workspace::{ProjectTrust, TRUST_PROJECT_OPENAPI_ENV},
};

/// Default `timeout_secs`, the same budget an MCP tool call gets.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Project sources live here, relative to the project root.
const PROJECT_FILE: &str = ".rho/openapi.toml";

/// Every `[openapi.<source>]` table, keyed by source name.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub(crate) struct OpenApiConfig {
    pub(crate) sources: BTreeMap<String, OpenApiSourceConfig>,
}

impl OpenApiConfig {
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// One source as written in config.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenApiSourceConfig {
    /// HTTP(S) URL or file path of a JSON or YAML OpenAPI 3.0/3.1 spec.
    pub(crate) spec: String,
    /// Replaces every server the spec declares. Required with credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    /// Literal headers sent to the `base_url` origin.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) headers: BTreeMap<String, String>,
    /// Header names mapped to environment variable names. User config only.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) headers_from_env: BTreeMap<String, String>,
    /// One credential for every request to the `base_url` origin. User config only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) auth: Option<AuthConfig>,
    /// Globs on the operationId or `METHOD /path`. Empty allows everything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) allow: Vec<String>,
    /// Globs on the operationId or `METHOD /path`, applied before `allow`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_secs: Option<u64>,
}

impl OpenApiSourceConfig {
    /// Whether requests carry anything that may be secret. Literal headers
    /// count: `Authorization = "Bearer ..."` is a secret written inline.
    fn has_credentials(&self) -> bool {
        self.auth.is_some() || !self.headers_from_env.is_empty() || !self.headers.is_empty()
    }
}

/// Where a credential comes from. Values are always read from the
/// environment, never written in config.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AuthConfig {
    /// `Authorization: Bearer <token>`.
    Bearer { token_env: String },
    /// A key sent as one header or query parameter.
    ApiKey {
        value_env: String,
        #[serde(rename = "in")]
        location: ApiKeyLocation,
        name: String,
    },
    /// `Authorization: Basic <base64(username:password)>`.
    Basic {
        username_env: String,
        password_env: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApiKeyLocation {
    Header,
    Query,
}

/// Which file declared a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigOrigin {
    User,
    Project,
}

/// Sources a session should load, read from both config files.
#[derive(Clone, Debug, Default)]
pub(crate) struct OpenApiSources {
    pub(crate) user: OpenApiConfig,
    /// Directory relative user `spec` paths resolve against.
    pub(crate) user_dir: PathBuf,
    /// The project file, when one exists.
    pub(crate) project: Option<ProjectSources>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectSources {
    /// An untrusted workspace's file is reported and skipped: its server
    /// gets approval-free GETs, which may reach internal addresses.
    pub(crate) trust: ProjectTrust,
    pub(crate) path: PathBuf,
    /// The project root, which relative `spec` paths resolve against.
    pub(crate) root: PathBuf,
    /// The parsed file, or why it could not be read.
    pub(crate) config: Result<OpenApiConfig, String>,
}

impl OpenApiSources {
    /// User sources from `config`, plus the project file for `cwd` if any.
    pub(crate) fn discover(
        user: &OpenApiConfig,
        config_path: &Path,
        cwd: &Path,
        trust: ProjectTrust,
    ) -> Self {
        let root = crate::plugins::state::project_root(cwd);
        let path = root.join(PROJECT_FILE);
        let project = match std::fs::read_to_string(&path) {
            Ok(text) => Some(ProjectSources {
                trust,
                config: parse_project_file(&text),
                path,
                root,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(ProjectSources {
                trust,
                config: Err(format!("could not read: {error}")),
                path,
                root,
            }),
        };
        Self {
            user: user.clone(),
            user_dir: config_path
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf),
            project,
        }
    }

    /// Validate every source and resolve its credentials through `env`.
    /// Returns the sources to load and a failure for each one that cannot.
    pub(crate) fn plan(&self, env: &dyn Fn(&str) -> Option<String>) -> SourcePlans {
        let mut plans = SourcePlans::default();
        for (name, source) in &self.user.sources {
            plans.push(
                name,
                plan_source(name, source, &self.user_dir, ConfigOrigin::User, env),
            );
        }
        let Some(project) = &self.project else {
            return plans;
        };
        match project.trust {
            ProjectTrust::Trusted => {}
            ProjectTrust::Untrusted => {
                plans.warnings.push(format!(
                    "ignoring {} because this workspace is not trusted; set {TRUST_PROJECT_OPENAPI_ENV}=1 to load it",
                    crate::paths::display(&project.path)
                ));
                return plans;
            }
        }
        let config = match &project.config {
            Ok(config) => config,
            Err(error) => {
                plans.failures.push(SourceFailure {
                    name: None,
                    error: format!("{}: {error}", project.path.display()),
                });
                return plans;
            }
        };
        let origin = ConfigOrigin::Project;
        for (name, source) in &config.sources {
            if self.user.sources.contains_key(name) {
                plans.warnings.push(format!(
                    "OpenAPI source `{name}` in {} is ignored: user config declares the same source",
                    project.path.display()
                ));
                continue;
            }
            plans.push(
                name,
                plan_source(name, source, &project.root, origin, env)
                    .map_err(|error| format!("{}: {error}", project.path.display())),
            );
        }
        plans
    }
}

fn parse_project_file(text: &str) -> Result<OpenApiConfig, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ProjectFile {
        #[serde(default)]
        openapi: OpenApiConfig,
    }
    toml::from_str::<ProjectFile>(text)
        .map(|file| file.openapi)
        .map_err(|error| error.to_string())
}

/// The planned sources and the ones that failed validation.
#[derive(Debug, Default)]
pub(crate) struct SourcePlans {
    pub(crate) sources: Vec<SourcePlan>,
    pub(crate) failures: Vec<SourceFailure>,
    pub(crate) warnings: Vec<String>,
}

impl SourcePlans {
    /// Whether there is nothing to load or report.
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.failures.is_empty() && self.warnings.is_empty()
    }

    fn push(&mut self, name: &str, plan: Result<SourcePlan, String>) {
        match plan {
            Ok(plan) => self.sources.push(plan),
            Err(error) => self.failures.push(SourceFailure {
                name: Some(name.to_owned()),
                error,
            }),
        }
    }
}

/// A source that will not load, and why. `name` is `None` when a whole
/// config file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceFailure {
    pub(crate) name: Option<String>,
    pub(crate) error: String,
}

/// A validated source with its credentials resolved.
#[derive(Clone, Debug)]
pub(crate) struct SourcePlan {
    pub(crate) name: String,
    pub(crate) spec: SpecLocation,
    pub(crate) base_url: Option<Url>,
    /// Sent only to the source origin; see [`SourceHeaders`].
    pub(crate) headers: SourceHeaders,
    pub(crate) filter: OperationFilter,
    pub(crate) timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SpecLocation {
    Url(Url),
    Path(PathBuf),
}

/// Headers and query credentials the source attaches to its own origin only.
/// Configured headers count too: a literal header may still be a secret.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SourceHeaders {
    pub(crate) headers: HeaderMap,
    /// An `api_key` credential sent `in = "query"`.
    pub(crate) query: Option<(String, String)>,
}

fn plan_source(
    name: &str,
    source: &OpenApiSourceConfig,
    base_dir: &Path,
    origin: ConfigOrigin,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<SourcePlan, String> {
    if name.is_empty() || sanitize_segment(name) != name {
        return Err(format!(
            "source name `{name}` must be ASCII letters and digits, optionally joined by single underscores"
        ));
    }
    match origin {
        ConfigOrigin::User => {}
        ConfigOrigin::Project => {
            let mut rejected = Vec::new();
            if source.auth.is_some() {
                rejected.push("`auth`");
            }
            if !source.headers_from_env.is_empty() {
                rejected.push("`headers_from_env`");
            }
            if !rejected.is_empty() {
                return Err(format!(
                "source `{name}` sets {} in project config; credentials come only from user config, \
                 so declare `[openapi.{name}]` in ~/.rho/config.toml instead",
                rejected.join(" and ")
            ));
            }
        }
    }
    let base_url = source
        .base_url
        .as_deref()
        .map(|base_url| parse_base_url(name, base_url))
        .transpose()?;
    if source.has_credentials() {
        let Some(base_url) = &base_url else {
            return Err(format!(
                "source `{name}` sends credentials or headers, so it needs a `base_url`: they go only to that origin"
            ));
        };
        crate::tools::mcp::validate::parse_secure_url(
            base_url.as_str(),
            &format!("source `{name}` base_url, which carries credentials,"),
        )
        .map_err(|error| error.to_string())?;
    }
    let timeout = match source.timeout_secs {
        None => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        Some(0) => return Err(format!("source `{name}` timeout_secs must be at least 1")),
        Some(seconds) => Duration::from_secs(seconds),
    };
    Ok(SourcePlan {
        name: name.to_owned(),
        spec: spec_location(name, &source.spec, base_dir)?,
        base_url,
        headers: resolve_headers(name, source, env)?,
        filter: OperationFilter::new(&source.allow, &source.deny),
        timeout,
    })
}

fn parse_base_url(name: &str, base_url: &str) -> Result<Url, String> {
    let url = Url::parse(base_url)
        .map_err(|error| format!("source `{name}` base_url `{base_url}` is invalid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(format!(
            "source `{name}` base_url `{base_url}` must be an HTTP(S) URL with a host"
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "source `{name}` base_url `{base_url}` must not have a query or fragment"
        ));
    }
    Ok(url)
}

fn spec_location(name: &str, spec: &str, base_dir: &Path) -> Result<SpecLocation, String> {
    if spec.trim().is_empty() {
        return Err(format!("source `{name}` spec must not be empty"));
    }
    if let Ok(url) = Url::parse(spec) {
        return match url.scheme() {
            "http" | "https" => Ok(SpecLocation::Url(url)),
            "file" => url
                .to_file_path()
                .map(SpecLocation::Path)
                .map_err(|()| format!("source `{name}` spec `{spec}` is not a local file URL")),
            // A Windows path such as `C:\specs\api.yaml` parses as scheme `c`.
            _ if Path::new(spec).is_absolute() => Ok(SpecLocation::Path(PathBuf::from(spec))),
            scheme => Err(format!(
                "source `{name}` spec `{spec}` uses unsupported scheme `{scheme}`"
            )),
        };
    }
    Ok(SpecLocation::Path(base_dir.join(spec)))
}

/// Literal headers first, then environment headers, then `auth`; later
/// entries replace earlier ones with the same name.
fn resolve_headers(
    name: &str,
    source: &OpenApiSourceConfig,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<SourceHeaders, String> {
    let mut resolved = SourceHeaders::default();
    let mut insert = |header: &str, value: &str, what: &str| -> Result<(), String> {
        let header = HeaderName::try_from(header)
            .map_err(|_| format!("source `{name}` {what} `{header}` is not a valid header name"))?;
        let mut value = HeaderValue::try_from(value)
            .map_err(|_| format!("source `{name}` {what} `{header}` has an invalid value"))?;
        value.set_sensitive(true);
        resolved.headers.insert(header, value);
        Ok(())
    };
    for (header, value) in &source.headers {
        insert(header, value, "header")?;
    }
    let read = |variable: &str, field: &str| {
        env(variable).ok_or_else(|| {
            format!(
                "source `{name}` {field} names environment variable `{variable}`, which is not set"
            )
        })
    };
    for (header, variable) in &source.headers_from_env {
        insert(header, &read(variable, "headers_from_env")?, "header")?;
    }
    match &source.auth {
        None => {}
        Some(AuthConfig::Bearer { token_env }) => {
            let token = read(token_env, "auth.token_env")?;
            insert("authorization", &format!("Bearer {token}"), "auth")?;
        }
        Some(AuthConfig::Basic {
            username_env,
            password_env,
        }) => {
            use base64::Engine as _;
            let username = read(username_env, "auth.username_env")?;
            let password = read(password_env, "auth.password_env")?;
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            insert("authorization", &format!("Basic {encoded}"), "auth")?;
        }
        Some(AuthConfig::ApiKey {
            value_env,
            location,
            name: key,
        }) => {
            if key.is_empty() {
                return Err(format!("source `{name}` auth.name must not be empty"));
            }
            let value = read(value_env, "auth.value_env")?;
            match location {
                ApiKeyLocation::Header => insert(key, &value, "auth.name")?,
                ApiKeyLocation::Query => resolved.query = Some((key.clone(), value)),
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
