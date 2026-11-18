//! The watch tools as MCP publishes them: `watch` and `unwatch`, and the
//! `notify` argument the HTTP adapter adds to `new` and `prompt`. The core
//! never sees `notify`: [`take_notify`] strips it before the call, since the
//! core's params refuse unknown fields.

use rho_protocol::SessionId;
use rmcp::model::{Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::app::{
    mcp_server::tool::ServedTool,
    session_control::{schema_of, Action, ControlError},
};

const NOTIFY: &str = "notify";

/// Params of `watch`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WatchParams {
    pub(super) session: SessionId,
    /// The webhook to POST the session's news to, by name; one bound to
    /// the caller's token.
    pub(super) webhook: String,
}

/// Params of `unwatch`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UnwatchParams {
    pub(super) session: SessionId,
}

/// `watch` and `unwatch`, each with the tool it is.
pub(crate) fn tools() -> [(ServedTool, Tool); 2] {
    let (watch, unwatch) = (ServedTool::Watch.name(), ServedTool::Unwatch.name());
    [
        (
            ServedTool::Watch,
            Tool::new(
                watch,
                "Have this server POST a rho session's news to a webhook: each time it stops to \
             wait for a person, and once when it settles, which ends the watch. Returns where \
             the session is now, as `wait` does; when it settled already, that is the news \
             and this call adds no watch; `watching` says whether one is left, such as one a \
             concurrent `prompt` with `notify` set. `notify` on the next `prompt` watches \
             again. Calling it again replaces the session's watch. `watch_error`, when present, \
             says the server could not save the change, so a restart of the server loses it.",
                schema_of::<WatchParams>(),
            )
            .with_annotations(ToolAnnotations::with_title(watch)),
        ),
        (
            ServedTool::Unwatch,
            Tool::new(
                unwatch,
                "Stop watching a rho session; nothing more is POSTed for it.",
                schema_of::<UnwatchParams>(),
            )
            .with_annotations(ToolAnnotations::with_title(unwatch).idempotent(true)),
        ),
    ]
}

/// `action`'s input schema, with `notify` for the actions that start a
/// turn.
pub(crate) fn with_notify(action: Action, mut schema: Map<String, Value>) -> Map<String, Value> {
    if matches!(action, Action::New | Action::Prompt) {
        if let Some(Value::Object(properties)) = schema.get_mut("properties") {
            properties.insert(
                NOTIFY.to_owned(),
                serde_json::json!({
                    "type": "string",
                    "description": "A webhook, by name, to POST the session's news to, as \
                        `watch` does, starting when this turn starts. Once the turn starts the \
                        call succeeds; `watch_error`, when present, says what went wrong with \
                        the watch.",
                }),
            );
        }
    }
    schema
}

/// Removes `notify` from the arguments of an action that starts a turn:
/// the webhook it names, if any. Any other action's arguments are left for
/// the core, which refuses the field.
pub(super) fn take_notify(
    action: Action,
    arguments: &mut Map<String, Value>,
) -> Result<Option<String>, ControlError> {
    if !matches!(action, Action::New | Action::Prompt) {
        return Ok(None);
    }
    match arguments.remove(NOTIFY) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(webhook)) => Ok(Some(webhook)),
        Some(other) => Err(ControlError::Invalid(format!(
            "invalid `{}` arguments: `{NOTIFY}` is a webhook name, not {other}",
            action.name()
        ))),
    }
}
