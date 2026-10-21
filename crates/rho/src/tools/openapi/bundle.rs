//! Loading configured sources into one bundle of read-only tools.
//!
//! Only GET and HEAD operations become tools; write methods are not
//! registered at all. The tools stay out of model context until `tool_search`
//! loads them, the same as MCP tools, so a 600-operation API costs one prompt
//! line rather than its whole catalog.

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use reqwest::Method;
use rho_sdk::{model::ToolSpec, tool::Tool};
use url::Url;

use super::{
    config::{SourceFailure, SourcePlan, SourcePlans},
    fetch::{SpecCache, SpecFetch},
    operation::{HttpMethod, Operation, ResponseBody},
    report::{LoadedSource, OpenApiReport},
    request::RequestShape,
    response::MAX_RESPONSE_BYTES,
    tool::{OpenApiTool, SourceClient},
    OpenApiSpec, SpecInput, DEFAULT_MAX_SPEC_BYTES,
};
use crate::tools::{code_mode::ContextExposure, sdk_registry::ToolBundle};

/// The tools of every loaded source.
pub(crate) struct OpenApiBundle {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolBundle for OpenApiBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    /// GitHub alone has 651 read operations, so none cost context until
    /// `tool_search` loads them.
    fn context_exposure(&self) -> ContextExposure {
        ContextExposure::Hidden
    }
}

/// The bundle, if any source loaded, and what to tell the user and model.
#[derive(Default)]
pub(crate) struct OpenApiLoadOutcome {
    pub(crate) bundle: Option<OpenApiBundle>,
    pub(crate) report: OpenApiReport,
}

/// Session inputs every source shares.
pub(crate) struct LoadOptions {
    pub(crate) cache: Option<SpecCache>,
    pub(crate) max_output_bytes: usize,
    pub(crate) max_response_bytes: usize,
}

impl LoadOptions {
    pub(crate) fn new(max_output_bytes: usize) -> Self {
        Self {
            cache: SpecCache::default_dir().map(SpecCache::new),
            max_output_bytes,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
}

/// Fetch, parse, and index every planned source in parallel.
pub(crate) async fn load(plans: SourcePlans, options: LoadOptions) -> OpenApiLoadOutcome {
    let SourcePlans {
        sources,
        failures,
        warnings,
    } = plans;
    let mut report = OpenApiReport {
        sources: Vec::new(),
        errors: failures.into_iter().map(failure_message).collect(),
        warnings,
    };
    if sources.is_empty() {
        return OpenApiLoadOutcome {
            bundle: None,
            report,
        };
    }
    let clients = SourceClient::http_client().and_then(|calls| {
        let specs = crate::reqwest_client_builder()
            .user_agent(super::tool::USER_AGENT)
            .build()?;
        Ok((calls, specs))
    });
    let (calls, specs) = match clients {
        Ok(clients) => clients,
        Err(error) => {
            report.errors.push(format!(
                "could not load OpenAPI sources: could not build an HTTP client: {error}"
            ));
            return OpenApiLoadOutcome {
                bundle: None,
                report,
            };
        }
    };
    let jobs = sources
        .into_iter()
        .map(|plan| load_source(plan, &calls, &specs, &options));
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut names = HashSet::new();
    for (name, result) in futures_util::future::join_all(jobs).await {
        match result {
            Ok(loaded) => {
                report.warnings.extend(loaded.warning);
                for tool in loaded.tools {
                    let tool_name = tool.spec().name;
                    if names.insert(tool_name.clone()) {
                        tools.push(tool);
                    } else {
                        report.warnings.push(format!(
                            "OpenAPI tool `{tool_name}` is declared twice; keeping the first"
                        ));
                    }
                }
                report.sources.push(loaded.source);
            }
            Err(error) => report
                .errors
                .push(format!("could not load OpenAPI source `{name}`: {error}")),
        }
    }
    OpenApiLoadOutcome {
        bundle: (!tools.is_empty()).then_some(OpenApiBundle { tools }),
        report,
    }
}

fn failure_message(failure: SourceFailure) -> String {
    match failure.name {
        Some(name) => format!("could not load OpenAPI source `{name}`: {}", failure.error),
        None => format!("could not load OpenAPI sources: {}", failure.error),
    }
}

struct Loaded {
    tools: Vec<Arc<dyn Tool>>,
    source: LoadedSource,
    warning: Option<String>,
}

async fn load_source(
    plan: SourcePlan,
    calls: &reqwest::Client,
    specs: &reqwest::Client,
    options: &LoadOptions,
) -> (String, Result<Loaded, String>) {
    let name = plan.name.clone();
    let fetch = SpecFetch {
        client: specs,
        cache: options.cache.as_ref(),
        timeout: plan.timeout,
        max_spec_bytes: DEFAULT_MAX_SPEC_BYTES,
    };
    let fetched = match fetch.fetch(&plan.spec).await {
        Ok(fetched) => fetched,
        Err(error) => return (name, Err(error)),
    };
    let cache = options.cache.clone();
    let source = SourceSettings {
        calls: calls.clone(),
        max_output_bytes: options.max_output_bytes,
        max_response_bytes: options.max_response_bytes,
    };
    // Parsing GitHub's 13.5 MB spec and rendering 651 input schemas is CPU
    // work that must not stall the runtime.
    let built = tokio::task::spawn_blocking(move || {
        let spec = OpenApiSpec::load(SpecInput {
            name: &plan.name,
            url: &fetched.url,
            bytes: &fetched.bytes,
            max_spec_bytes: DEFAULT_MAX_SPEC_BYTES,
        })
        .map_err(|error| error.to_string())?;
        if let Some(cache) = &cache {
            cache.remember(&fetched);
        }
        for skipped in &spec.skipped {
            tracing::debug!(
                source = %plan.name,
                path = %skipped.path,
                reason = %skipped.reason,
                "OpenAPI operation skipped"
            );
        }
        let (tools, source) = build_tools(&plan, &spec, source);
        Ok(Loaded {
            tools,
            source,
            warning: fetched
                .warning
                .map(|warning| format!("OpenAPI source `{}`: {warning}", plan.name)),
        })
    })
    .await
    .unwrap_or_else(|error| Err(format!("loading stopped: {error}")));
    (name, built)
}

struct SourceSettings {
    calls: reqwest::Client,
    max_output_bytes: usize,
    max_response_bytes: usize,
}

fn build_tools(
    plan: &SourcePlan,
    spec: &OpenApiSpec,
    settings: SourceSettings,
) -> (Vec<Arc<dyn Tool>>, LoadedSource) {
    let default_server = spec.servers.iter().find_map(|server| server.as_ref().ok());
    // Without a `base_url` the source sends no credentials or headers, so
    // this origin only decides where nothing is attached.
    let origin = plan
        .base_url
        .as_ref()
        .or(default_server)
        .map_or_else(url::Origin::new_opaque, Url::origin);
    let source = Arc::new(SourceClient {
        name: plan.name.clone(),
        client: settings.calls,
        origin,
        headers: plan.headers.clone(),
        timeout: plan.timeout,
        max_output_bytes: settings.max_output_bytes,
        max_response_bytes: settings.max_response_bytes,
    });
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    let mut tag_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for operation in &spec.operations {
        let method = match operation.method {
            HttpMethod::Get => Method::GET,
            HttpMethod::Head => Method::HEAD,
            HttpMethod::Put
            | HttpMethod::Post
            | HttpMethod::Delete
            | HttpMethod::Options
            | HttpMethod::Patch
            | HttpMethod::Trace => continue,
        };
        if !plan.filter.admits(operation) {
            continue;
        }
        // An operation that declares its own servers never falls back to the
        // spec's: if none of them is usable, it has nowhere to go.
        let base = plan.base_url.as_ref().or_else(|| match &operation.servers {
            Some(servers) => servers.iter().find_map(|server| server.as_ref().ok()),
            None => default_server,
        });
        let Some(base) = base else {
            tracing::debug!(source = %plan.name, path = %operation.path, "OpenAPI operation has no usable server");
            continue;
        };
        let (shape, input_schema) = match RequestShape::new(operation, &spec.document) {
            Ok(built) => built,
            Err(error) => {
                tracing::debug!(source = %plan.name, path = %operation.path, error = %error, "OpenAPI operation schema did not render");
                continue;
            }
        };
        let route = format!("{} {}", method.as_str(), operation.path);
        for tag in &operation.tags {
            *tag_counts.entry(tag).or_default() += 1;
        }
        tools.push(Arc::new(OpenApiTool {
            spec: ToolSpec {
                name: operation.tool_name.clone(),
                description: description(&plan.name, &route, operation),
                input_schema,
            },
            source: source.clone(),
            method,
            route,
            base: base.clone(),
            path: operation.path.clone(),
            shape,
            accept: accept(operation),
        }));
    }
    let mut tags: Vec<(&str, usize)> = tag_counts.into_iter().collect();
    tags.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    let loaded = LoadedSource {
        name: plan.name.clone(),
        read_operations: tools.len(),
        tags: tags.into_iter().map(|(tag, _)| tag.to_owned()).collect(),
    };
    (tools, loaded)
}

/// What search matches on: the summary, route, source, and tags. Codemode
/// search weights the first paragraph above the rest, so the summary leads,
/// separated by a blank line. Spec text is untrusted, like MCP tool
/// descriptions.
fn description(source: &str, route: &str, operation: &Operation) -> String {
    let mut text = String::new();
    if let Some(summary) = operation.summary.as_deref().map(str::trim) {
        if !summary.is_empty() {
            text.push_str(summary);
            text.push_str("\n\n");
        }
    }
    text.push_str(&format!("{route} on OpenAPI source `{source}`"));
    if !operation.tags.is_empty() {
        text.push_str(&format!(" (tags: {})", operation.tags.join(", ")));
    }
    text.push('.');
    text
}

/// The media type to ask for: the first JSON or text success response.
fn accept(operation: &Operation) -> Option<String> {
    operation
        .responses
        .iter()
        .find_map(|response| match &response.body {
            ResponseBody::Json(content) if content.media_type == "*/*" => {
                Some("application/json".to_owned())
            }
            ResponseBody::Json(content) => Some(content.media_type.clone()),
            ResponseBody::Text { media_type } => Some(media_type.clone()),
            ResponseBody::Empty => None,
        })
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
