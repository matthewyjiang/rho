//! The `ui` tool: shows the user an A2UI surface the model wrote in A2UI
//! Express, and can wait for what the user does on it.
//!
//! The surface reaches clients as presentation metadata, which the model
//! never reads: its result is the surface's id, or the user's action.

use std::sync::Arc;

use rho_sdk::{
    model::ToolSpec,
    tool::{
        Tool, ToolAsset, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation,
        ToolMetadata, ToolOutput, ToolProgress, ToolSecurity,
    },
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{code_mode::ContextExposure, sdk_registry::ToolBundle};

mod host;

pub(crate) use host::{SurfaceAction, SurfaceContent, SurfaceHost, SurfaceRequest};

pub(crate) const TOOL_NAME: &str = "ui";

/// The longest Express block a call takes.
const MAX_EXPRESS_BYTES: usize = 64 * 1024;

/// The longest surface id a call may name.
const MAX_SURFACE_ID_BYTES: usize = 64;

/// The media type of the [`ToolAsset`] that carries what a call showed: a
/// [`rho_protocol::ToolContent::Surface`] as JSON.
const SURFACE_MEDIA_TYPE: &str = "application/vnd.rho.a2ui-surface+json";

/// How the error of a call that showed its surface and then stopped
/// waiting on it begins.
const STOPPED_WAITING: &str = "stopped waiting on surface ";

fn stopped_waiting(surface: &str, why: &str) -> ToolError {
    ToolError::new(
        ToolErrorKind::Execution,
        format!("{STOPPED_WAITING}`{surface}`: {why}"),
    )
}

/// The surface `metadata` carries, if a `ui` call reported one.
pub(crate) fn shown_surface(metadata: &ToolMetadata) -> Option<rho_protocol::ToolContent> {
    metadata
        .assets()
        .iter()
        .find(|asset| asset.media_type() == SURFACE_MEDIA_TYPE)
        .and_then(|asset| serde_json::from_slice(asset.bytes()).ok())
}

/// A `ui` call's arguments.
#[derive(Debug)]
struct Arguments {
    request: SurfaceRequest,
    wait: bool,
}

impl Arguments {
    fn parse(arguments: &Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            express: String,
            #[serde(default)]
            surface: Option<String>,
            #[serde(default)]
            wait: bool,
        }
        let Raw {
            express,
            surface,
            wait,
        } = Raw::deserialize(arguments).map_err(|error| error.to_string())?;
        if express.trim().is_empty() {
            return Err("express must be a non-empty A2UI Express block".to_owned());
        }
        if express.len() > MAX_EXPRESS_BYTES {
            return Err(format!(
                "express is {} bytes; the limit is {MAX_EXPRESS_BYTES}",
                express.len()
            ));
        }
        if let Some(surface) = &surface {
            let plain = surface
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
            if surface.is_empty() || surface.len() > MAX_SURFACE_ID_BYTES || !plain {
                return Err(format!(
                    "surface must be 1 to {MAX_SURFACE_ID_BYTES} letters, digits, `-`, `_` or `.`"
                ));
            }
        }
        Ok(Self {
            request: SurfaceRequest { express, surface },
            wait,
        })
    }
}

struct UiTool {
    surfaces: SurfaceHost,
    description: String,
}

impl Tool for UiTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: TOOL_NAME.into(),
            description: self.description.clone(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "express": {
                        "type": "string",
                        "description": "The A2UI Express block.",
                    },
                    "surface": {
                        "type": "string",
                        "description": "The block's default surface: the id to create, or the surface to update. Omit it to create a surface under a new id.",
                    },
                    "wait": {
                        "type": "boolean",
                        "description": "Return the user's next action on the surface instead of returning at once. Default false.",
                    },
                },
                "required": ["express"],
                "additionalProperties": false,
            }),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let call_id = invocation.id().clone();
            let invalid = |message| ToolError::new(ToolErrorKind::InvalidArguments, message);
            let Arguments { request, wait } =
                Arguments::parse(&invocation.into_arguments()).map_err(invalid)?;
            let (content, waiting) = if wait {
                let (content, waiting) = self
                    .surfaces
                    .show_waiting(call_id.as_str(), &request)
                    .map_err(invalid)?;
                (content, Some(waiting))
            } else {
                let content = self
                    .surfaces
                    .show(call_id.as_str(), &request)
                    .map_err(invalid)?;
                (content, None)
            };
            let shown = serde_json::to_vec(&content.wire())
                .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))?;
            let metadata = ToolMetadata::new().asset(ToolAsset::new(SURFACE_MEDIA_TYPE, shown));
            let Some(mut waiting) = waiting else {
                return Ok(
                    ToolOutput::text(json!({ "surfaceId": content.surface }).to_string())
                        .metadata(metadata),
                );
            };
            // The user has to see the surface to act on it, so clients get
            // it now, while the call still runs.
            context
                .progress()
                .send(
                    ToolProgress::new(format!("waiting on surface {}", content.surface))
                        .with_metadata(metadata.clone()),
                )
                .await;
            // An action that arrived as the turn was cancelled goes back
            // to the session when `waiting` drops.
            let action = tokio::select! {
                biased;
                () = context.cancellation().cancelled() => return Err(ToolError::cancelled()),
                action = waiting.action() => action.map_err(|_| {
                    stopped_waiting(&content.surface, "another call waits on it now, or the conversation moved to another branch")
                })?,
            };
            Ok(ToolOutput::text(action.action.to_string()).metadata(metadata))
        })
    }
}

/// The `ui` tool over `surfaces`. Its description is the Express prompt,
/// some 16,000 characters, so it stays out of context until `tool_search`
/// loads it.
pub(super) struct UiBundle {
    tools: Vec<Arc<dyn Tool>>,
}

impl UiBundle {
    pub(super) fn new(surfaces: SurfaceHost) -> Self {
        let description = format!(
            "Show the user a small interface in the transcript, such as a confirmation, a form \
             or a result card, written in A2UI Express, and optionally wait for what they do on \
             it.\n\n\
             One call shows or updates one surface. The `surface` argument names the block's \
             default surface; without it a new surface gets an id, which the result returns as \
             `surfaceId`. With `wait: true` the result is the user's next action on the surface \
             instead: its `name`, `sourceComponentId` and resolved `context`. Give each button \
             an event action, so the action tells you what the user chose. An action taken \
             after the call returned reaches you later, as a message naming the surface. \
             Function calls to the client are not available.\n\n{}",
            host::express_prompt()
        );
        Self {
            tools: vec![Arc::new(UiTool {
                surfaces,
                description,
            })],
        }
    }
}

impl ToolBundle for UiBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    fn context_exposure(&self) -> ContextExposure {
        ContextExposure::Hidden
    }
}
