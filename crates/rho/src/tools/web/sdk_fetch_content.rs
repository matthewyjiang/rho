use rho_sdk::{
    tool::{
        OperationKind, Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
        ToolMetadata, ToolOutput, ToolSecurity,
    },
    CapabilityKind, CapabilityRequest, CapabilitySource, NetworkTarget, ResolvedWorkspacePath,
    WorkspacePathError, WorkspacePathErrorKind,
};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use rho_tools::tool::truncate;

use super::{
    fetch::{self, github, FetchedTarget},
    storage::{self, StoredContent, StoredItem},
    util::{self, to_pretty_json},
};

mod github_clone;

use github_clone::GitHubClonePlan;

const DEFAULT_FRAMES: usize = 6;
const FETCH_CONTENT_TOOL: &str = "fetch_content";

pub(in crate::tools) struct SdkFetchContent {
    client: reqwest::Client,
    max_output_bytes: usize,
    access: super::guard::NetworkAccess,
}

impl SdkFetchContent {
    pub(in crate::tools) fn new(
        max_output_bytes: usize,
        access: super::guard::NetworkAccess,
    ) -> Self {
        Self {
            client: util::fetch_http_client(access),
            max_output_bytes,
            access,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchContentArgs {
    url: Option<String>,
    urls: Option<Vec<String>>,
    prompt: Option<String>,
    timestamp: Option<String>,
    frames: Option<usize>,
    force_clone: Option<bool>,
}

struct TargetOptions<'a> {
    response_id: &'a str,
    target_index: usize,
    prompt: Option<&'a str>,
    timestamp: Option<&'a str>,
    frames: usize,
    force_clone: bool,
    max_output_bytes: usize,
}

struct FetchPlan {
    response_id: String,
    prompt: Option<String>,
    targets: Vec<TargetPlan>,
}

enum TargetPlan {
    Local(LocalPlan),
    Http(HttpPlan),
    Placeholder(PlaceholderPlan),
    GitHubApi(GitHubApiPlan),
    GitHubClone(GitHubClonePlan),
}

struct LocalPlan {
    requested: String,
    resolved: ResolvedWorkspacePath,
    prompt: Option<String>,
    timestamp: Option<String>,
    frames: usize,
}

struct HttpPlan {
    requested: String,
    url: Url,
    prompt: Option<String>,
}

struct PlaceholderPlan {
    requested: String,
    kind: PlaceholderKind,
    prompt: Option<String>,
    timestamp: Option<String>,
    frames: usize,
}

enum PlaceholderKind {
    YouTube,
    RemotePdf,
}

struct GitHubApiPlan {
    requested: String,
    target: github::GitHubTarget,
    api_url: String,
}

impl FetchPlan {
    fn parse(
        arguments: Value,
        context: &ToolContext,
        max_output_bytes: usize,
    ) -> Result<Self, ToolError> {
        let arguments: FetchContentArgs = serde_json::from_value(arguments).map_err(|error| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                format!("invalid arguments: {error}"),
            )
        })?;
        let targets = collect_targets(arguments.url, arguments.urls)?;
        let frames = arguments.frames.unwrap_or(DEFAULT_FRAMES).clamp(1, 12);
        let force_clone = arguments.force_clone.unwrap_or(false);
        let response_id = storage::new_response_id();
        let workspace = context.workspace().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::Execution,
                "workspace is required for fetch_content",
            )
        })?;
        let mut plans = Vec::with_capacity(targets.len());
        for (target_index, target) in targets.into_iter().enumerate() {
            plans.push(TargetPlan::parse(
                target,
                workspace,
                TargetOptions {
                    response_id: &response_id,
                    target_index,
                    prompt: arguments.prompt.as_deref(),
                    timestamp: arguments.timestamp.as_deref(),
                    frames,
                    force_clone,
                    max_output_bytes,
                },
            )?);
        }
        Ok(Self {
            response_id,
            prompt: arguments.prompt,
            targets: plans,
        })
    }

    async fn authorize(&self, context: &ToolContext) -> Result<(), ToolError> {
        for target in &self.targets {
            target.authorize(context).await?;
        }
        Ok(())
    }

    async fn execute(
        self,
        client: &reqwest::Client,
        context: &ToolContext,
        max_output_bytes: usize,
        access: super::guard::NetworkAccess,
    ) -> Result<ToolOutput, ToolError> {
        let mut items = Vec::with_capacity(self.targets.len());
        let mut previews = Vec::with_capacity(self.targets.len());
        for target in self.targets {
            let requested = target.requested().to_owned();
            let fetched = target.execute(client, context, access).await?;
            previews.push(fetched.preview.clone());
            items.push(StoredItem {
                url: Some(requested),
                query: self.prompt.clone(),
                title: fetched.title,
                content: fetched.content,
                metadata: fetched.metadata,
            });
        }

        storage::store(
            self.response_id.clone(),
            StoredContent {
                kind: FETCH_CONTENT_TOOL.into(),
                items,
            },
        );
        let content = json!({
            "responseId": self.response_id,
            "type": FETCH_CONTENT_TOOL,
            "items": previews,
            "fullContentAvailable": true,
            "note": "Large fetched content is stored out-of-band. Use get_search_content with responseId to retrieve it."
        });
        Ok(
            ToolOutput::text(truncate(to_pretty_json(&content), max_output_bytes))
                .metadata(ToolMetadata::new().operation(OperationKind::Network)),
        )
    }
}

impl TargetPlan {
    fn parse(
        requested: String,
        workspace: &rho_sdk::Workspace,
        options: TargetOptions<'_>,
    ) -> Result<Self, ToolError> {
        if let Some(target) = github::parse_url(&requested) {
            if options.force_clone && target.kind != github::GitHubKind::Commit {
                return Ok(Self::GitHubClone(GitHubClonePlan::new(
                    requested,
                    target,
                    options.response_id,
                    options.target_index,
                    workspace.root(),
                    options.max_output_bytes,
                )));
            }
            let api_url = github::api_url(&target);
            return Ok(Self::GitHubApi(GitHubApiPlan {
                requested,
                target,
                api_url,
            }));
        }

        if util::is_youtube_url(&requested) {
            return Ok(Self::Placeholder(PlaceholderPlan {
                requested,
                kind: PlaceholderKind::YouTube,
                prompt: options.prompt.map(str::to_owned),
                timestamp: options.timestamp.map(str::to_owned),
                frames: options.frames,
            }));
        }

        if let Ok(url) = Url::parse(&requested) {
            if matches!(url.scheme(), "http" | "https") {
                if fetch::content_type_from_path(url.path()) == "pdf" {
                    return Ok(Self::Placeholder(PlaceholderPlan {
                        requested,
                        kind: PlaceholderKind::RemotePdf,
                        prompt: None,
                        timestamp: None,
                        frames: options.frames,
                    }));
                }
                return Ok(Self::Http(HttpPlan {
                    requested,
                    url,
                    prompt: options.prompt.map(str::to_owned),
                }));
            }
        }

        let resolved = workspace
            .resolve_for_read(&requested)
            .map_err(map_workspace_path_error)?;
        Ok(Self::Local(LocalPlan {
            requested,
            resolved,
            prompt: options.prompt.map(str::to_owned),
            timestamp: options.timestamp.map(str::to_owned),
            frames: options.frames,
        }))
    }

    fn requested(&self) -> &str {
        match self {
            Self::Local(plan) => &plan.requested,
            Self::Http(plan) => &plan.requested,
            Self::Placeholder(plan) => &plan.requested,
            Self::GitHubApi(plan) => &plan.requested,
            Self::GitHubClone(plan) => plan.requested(),
        }
    }

    async fn authorize(&self, context: &ToolContext) -> Result<(), ToolError> {
        match self {
            Self::Local(plan) => {
                authorize(
                    context,
                    CapabilityRequest::read_path(
                        plan.resolved.path(),
                        plan.resolved.scope().clone(),
                        capability_source(),
                    ),
                )
                .await
            }
            Self::Http(plan) => {
                authorize(
                    context,
                    CapabilityRequest::network(
                        NetworkTarget::Url(plan.url.as_str().to_owned()),
                        capability_source(),
                    ),
                )
                .await
            }
            Self::Placeholder(_) => Ok(()),
            Self::GitHubApi(plan) => {
                authorize(
                    context,
                    CapabilityRequest::network(
                        NetworkTarget::Url(plan.api_url.clone()),
                        capability_source(),
                    ),
                )
                .await
            }
            Self::GitHubClone(plan) => plan.authorize(context).await,
        }
    }

    async fn execute(
        self,
        client: &reqwest::Client,
        context: &ToolContext,
        access: super::guard::NetworkAccess,
    ) -> Result<FetchedTarget, ToolError> {
        match self {
            Self::Local(plan) => {
                let workspace = context.workspace().ok_or_else(|| {
                    ToolError::new(
                        ToolErrorKind::Execution,
                        "workspace is required for fetch_content",
                    )
                })?;
                workspace
                    .revalidate(&plan.resolved)
                    .map_err(map_workspace_path_error)?;
                fetch::fetch_local_path(
                    plan.resolved.path(),
                    plan.prompt.as_deref(),
                    plan.timestamp.as_deref(),
                    plan.frames,
                )
                .map_err(map_app_tool_error)
            }
            Self::Http(plan) => {
                fetch::fetch_http_url(client, &plan.url, plan.prompt.as_deref(), access)
                    .await
                    .map_err(map_app_tool_error)
            }
            Self::Placeholder(plan) => Ok(match plan.kind {
                PlaceholderKind::YouTube => fetch::youtube_placeholder(
                    &plan.requested,
                    plan.prompt.as_deref(),
                    plan.timestamp.as_deref(),
                    plan.frames,
                ),
                PlaceholderKind::RemotePdf => fetch::remote_pdf_fallback(&plan.requested),
            }),
            Self::GitHubApi(plan) => github::fetch_via_api(client, &plan.target)
                .await
                .map_err(map_app_tool_error),
            Self::GitHubClone(plan) => plan.execute().await,
        }
    }
}

impl Tool for SdkFetchContent {
    fn spec(&self) -> rho_sdk::model::ToolSpec {
        rho_sdk::model::ToolSpec {
            name: FETCH_CONTENT_TOOL.into(),
            description: "Fetch URLs, GitHub repos/files, YouTube/local videos, PDFs, local files, or web pages. Returns previews, artifacts, and responseId handles instead of dumping large content.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "urls": {"type": "array", "items": {"type": "string"}, "description": "URLs or local paths. Use one item for a single fetch, or multiple items to fetch several targets."},
                    "prompt": {"type": "string", "description": "Question for video or page analysis."},
                    "timestamp": {"type": "string", "description": "Video frame timestamp or range, e.g. 23:41 or 23:41-25:00."},
                    "frames": {"type": "integer", "minimum": 1, "maximum": 12},
                    "forceClone": {"type": "boolean", "description": "Clone GitHub repos even over the 350MB threshold."}
                },
                "required": ["urls"]
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([
            CapabilityKind::Read,
            CapabilityKind::Network,
            CapabilityKind::Process,
        ])
    }

    fn start_metadata(&self, _arguments: &Value) -> ToolMetadata {
        ToolMetadata::new().operation(OperationKind::Network)
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let plan =
                FetchPlan::parse(invocation.into_arguments(), &context, self.max_output_bytes)?;
            plan.authorize(&context).await?;
            plan.execute(&self.client, &context, self.max_output_bytes, self.access)
                .await
        })
    }
}

async fn authorize(context: &ToolContext, request: CapabilityRequest) -> Result<(), ToolError> {
    context
        .authorize(request)
        .await
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == rho_sdk::AuthorizationDenialKind::Cancelled {
                ToolError::cancelled()
            } else {
                ToolError::policy_denied(&error)
            }
        })
}

fn capability_source() -> CapabilitySource {
    CapabilitySource::built_in_tool(FETCH_CONTENT_TOOL)
}

fn collect_targets(
    singular: Option<String>,
    plural: Option<Vec<String>>,
) -> Result<Vec<String>, ToolError> {
    let targets = plural.or_else(|| singular.map(|value| vec![value]));
    match targets {
        Some(targets) if !targets.is_empty() => Ok(targets),
        _ => Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "fetch_content requires at least one URL or local path",
        )),
    }
}

fn map_workspace_path_error(error: WorkspacePathError) -> ToolError {
    let kind = match error.kind() {
        WorkspacePathErrorKind::ParentTraversal
        | WorkspacePathErrorKind::OutsideGrantedRoots
        | WorkspacePathErrorKind::InvalidPlatformPath
        | WorkspacePathErrorKind::ChangedAfterAuthorization => ToolErrorKind::PolicyDenied,
        _ => ToolErrorKind::Execution,
    };
    ToolError::new(kind, error.to_string())
}

fn map_app_tool_error(error: rho_tools::tool::ToolError) -> ToolError {
    let message = error.to_string();
    let kind = match error {
        rho_tools::tool::ToolError::InvalidArguments(_) => ToolErrorKind::InvalidArguments,
        rho_tools::tool::ToolError::Io(_)
        | rho_tools::tool::ToolError::Utf8(_)
        | rho_tools::tool::ToolError::Message(_) => ToolErrorKind::Execution,
    };
    ToolError::new(kind, message)
}

#[cfg(test)]
#[path = "sdk_fetch_content_tests.rs"]
mod tests;
