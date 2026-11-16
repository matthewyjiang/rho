//! A2UI surfaces: what a user does on one, sent back to its session.
//!
//! The `ui` tool shows the user a surface: its call carries the surface's
//! A2UI v1.0 messages as [`super::ToolContent::Surface`], which a client
//! that renders A2UI applies to its own A2UI processor and draws in the
//! transcript. When the user acts on it, by pressing a button for example,
//! the client sends the A2UI `action` message with
//! [`SESSION_SURFACE_ACTION`]. A client that cannot draw surfaces opens
//! its sessions without [`super::SessionPart::Surfaces`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SessionId;

pub const SESSION_SURFACE_ACTION: &str = "rho/session/surface_action";

/// Params of [`SESSION_SURFACE_ACTION`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSurfaceActionParams {
    pub session: SessionId,
    /// The surface the user acted on, as its
    /// [`super::ToolContent::Surface`] names it.
    pub surface: String,
    /// A2UI's renderer-to-agent `action` message, whole, as a client's
    /// A2UI processor writes it: `{"version": ..., "action": {"name",
    /// "surfaceId", "sourceComponentId", "timestamp", "context", ...}}`.
    /// The model reads its `action` member.
    pub action: serde_json::Value,
    /// The surface's data model, for a surface created with
    /// `sendDataModel`, as A2UI's transport attaches it to the message.
    /// The model reads it with the action, as `dataModel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_model: Option<serde_json::Value>,
}

/// Result of [`SESSION_SURFACE_ACTION`]: the session has the action. A
/// `ui` call waiting on the surface returns it; otherwise the model reads
/// it at its turn's next boundary, in a turn the daemon starts for it when
/// the session is idle and was opened with `idle_wake: turn`, or with the
/// next prompt. A surface the session does not have, an `action` that is
/// not A2UI's message for that surface, and a component with no such
/// action are [`crate::ErrorCode::InvalidParams`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSurfaceActionResponse {}
