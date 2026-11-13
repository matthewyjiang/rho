//! The `sessions` tool's control actions (#1714): `list`, `new`, `prompt`,
//! `steer`, `cancel`, `wait`, and `read` of a session the daemon hosts. A
//! thin adapter over the session-control core (`app::session_control`),
//! which `rho mcp serve` adapts too, so both answer the same call the same
//! way under one policy.
//!
//! The caller is this session's agent: its own holder, at the depth and
//! within the scope its environment carries (`crate::caller_context`), and
//! a session it starts records this one as its parent. The core checks a
//! call before anyone is asked to approve it; acting calls then ask for the
//! session-control capability naming the action, the session, its workspace,
//! and the text sent. Watching calls ask for the read of the session store
//! that searching history asks for, since they show other sessions'
//! transcripts. The core reaches only a daemon already running: a tool call
//! never starts one.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use rho_protocol::SessionId;
use rho_sdk::{
    tool::{
        OperationKind, PreparedToolInvocation, ToolError, ToolErrorKind, ToolMetadata, ToolOutput,
    },
    CapabilityRequest, CapabilitySource, PathScope, SessionControlAction, SessionControlTarget,
};
use serde_json::{json, Map, Value};

use crate::{
    app::session_control::{Action, Caller, ControlError, Limits, Reach, SessionControl, Target},
    caller_context::{Access, CallerContext},
    session_environment::SessionEnvironment,
};

/// A call of a core action: the action and its arguments, `action` removed.
pub(super) struct Call {
    action: Action,
    arguments: Map<String, Value>,
}

impl Call {
    /// The core call `arguments` make, if their action is the core's: every
    /// action but `search` and `recall`, and `read` without an `anchor`,
    /// which reads saved history instead.
    pub(super) fn parse(arguments: &Value) -> Option<Self> {
        let mut arguments = arguments.as_object()?.clone();
        let action = Action::from_name(arguments.get("action")?.as_str()?)?;
        if action == Action::Read && arguments.contains_key("anchor") {
            return None;
        }
        arguments.remove("action");
        Some(Self { action, arguments })
    }

    /// What the call shows before it runs, from its arguments alone.
    pub(super) fn start_metadata(&self) -> ToolMetadata {
        let named = |key| self.arguments.get(key).and_then(Value::as_str);
        metadata(
            self.action,
            named("session"),
            named("workspace").map(Path::new),
        )
    }
}

/// The operation acting calls report, beside reads.
const CONTROL_OPERATION: &str = "session control";

/// The core every session's tool in this process calls through, as one
/// `rho mcp serve` process has one: its caps count each holder, here each
/// calling session, across calls. The daemon, where sessions run, and an
/// in-process rho each run one async runtime, which the core's daemon
/// connection lives on.
fn core() -> &'static SessionControl {
    static CORE: OnceLock<SessionControl> = OnceLock::new();
    CORE.get_or_init(|| SessionControl::new(Limits::default(), Reach::Running))
}

/// Checks `call` from `session`, whose children see `environment`, and
/// prepares it. Acting calls ask for approval of what the check found;
/// watching calls read other sessions' transcripts, so they ask for the
/// read of the session store `root` that searching history asks for.
pub(super) async fn prepare<'a>(
    call: Call,
    session: &str,
    environment: &SessionEnvironment,
    root: &Result<PathBuf, String>,
) -> Result<PreparedToolInvocation<'a>, ToolError> {
    let ambient = environment.current();
    let context = CallerContext::from_env(|name| ambient.read_os(name))
        .map_err(|error| ToolError::new(ToolErrorKind::Execution, format!("{error:#}")))?;
    let caller = Caller::session(SessionId::new(session), &context, ambient.to_client());
    let Call { action, arguments } = call;
    let checked = core()
        .check(&caller, action, arguments)
        .await
        .map_err(tool_error)?;
    let source = CapabilitySource::built_in_tool(crate::session::search::TOOL_NAME);
    let (capability, metadata) = match checked.target() {
        None => {
            let root = root
                .clone()
                .map_err(|message| ToolError::new(ToolErrorKind::Execution, message))?;
            (
                CapabilityRequest::read_path(root, PathScope::UnrestrictedFilesystem, source),
                metadata(action, None, None),
            )
        }
        Some(target) => (
            CapabilityRequest::session_control(capability_target(target)?, source),
            metadata(
                action,
                target.session.as_ref().map(SessionId::as_str),
                Some(&target.workspace),
            ),
        ),
    };
    Ok(PreparedToolInvocation::resource_aware(
        [],
        [capability],
        metadata,
        move |context| {
            Box::pin(async move {
                let run = core().run(&caller, checked);
                let value = match action.needs() {
                    // A wait or read stops with the tool. An action runs to
                    // its end, so a session it opened is never left behind
                    // unreported, or uncounted against the caller's cap.
                    Access::Read => tokio::select! {
                        ran = run => ran,
                        () = context.cancellation().cancelled() => {
                            return Err(ToolError::new(ToolErrorKind::Cancelled, "cancelled"));
                        }
                    },
                    Access::Write => run.await,
                }
                .map_err(tool_error)?;
                serde_json::to_string_pretty(&value)
                    .map(ToolOutput::text)
                    .map_err(|error| ToolError::new(ToolErrorKind::Execution, error.to_string()))
            })
        },
    ))
}

/// Adds the core's actions to the history `schema`, with each action's
/// arguments as the core's own schema has them, so the tool and `rho mcp
/// serve` take the same arguments. An argument several actions take names
/// each action's use in its description.
pub(super) fn extend_schema(schema: &mut Value) {
    for action in Action::ALL {
        let mut own = Value::Object(action.input_schema());
        let defs = own.get("$defs").cloned().unwrap_or(Value::Null);
        inline_refs(&mut own, &defs);
        let name = action.name();
        let names = &mut schema["properties"]["action"]["enum"];
        if let Some(names) = names
            .as_array_mut()
            .filter(|names| !names.contains(&json!(name)))
        {
            names.push(json!(name));
        }
        let Some(arguments) = own.get_mut("properties").and_then(Value::as_object_mut) else {
            continue;
        };
        for (key, mut argument) in std::mem::take(arguments) {
            let said = argument
                .get("description")
                .and_then(Value::as_str)
                .map(|said| format!("For {name}: {said}"));
            let properties = &mut schema["properties"];
            match properties.get_mut(&key) {
                Some(known) => {
                    if let Some(said) = said {
                        known["description"] = json!(match known["description"].as_str() {
                            None | Some("") => said,
                            Some(before) if before.ends_with('.') => format!("{before} {said}"),
                            Some(before) => format!("{before}. {said}"),
                        });
                    }
                }
                None => {
                    if let Some(said) = said {
                        argument["description"] = json!(said);
                    }
                    properties[&key] = argument;
                }
            }
        }
    }
}

/// Replaces each `{"$ref": "#/$defs/Name"}` in `value` with `Name` from
/// `defs`, so an argument's schema stands alone in the merged schema.
fn inline_refs(value: &mut Value, defs: &Value) {
    if let Some(name) = value
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
    {
        let mut resolved = defs[name].clone();
        inline_refs(&mut resolved, defs);
        if let (Some(value), Value::Object(resolved)) = (value.as_object_mut(), resolved) {
            value.remove("$ref");
            for (key, field) in resolved {
                value.entry(key).or_insert(field);
            }
        }
        return;
    }
    match value {
        Value::Object(fields) => {
            fields.remove("$defs");
            fields
                .values_mut()
                .for_each(|field| inline_refs(field, defs));
        }
        Value::Array(items) => items.iter_mut().for_each(|item| inline_refs(item, defs)),
        _ => {}
    }
}

/// The capability target an approval names for `target`.
fn capability_target(target: &Target) -> Result<SessionControlTarget, ToolError> {
    let Some(session) = &target.session else {
        return Ok(SessionControlTarget::start(
            &target.workspace,
            target.text.clone().unwrap_or_default(),
        ));
    };
    let action = match target.action {
        Action::Prompt => SessionControlAction::Prompt,
        Action::Steer => SessionControlAction::Steer,
        Action::Cancel => SessionControlAction::Cancel,
        Action::New | Action::List | Action::Read | Action::Wait => {
            unreachable!("a target with a session is a prompt, steer, or cancel")
        }
    };
    let session = rho_sdk::SessionId::from_string(session.as_str())
        .map_err(|_| ToolError::new(ToolErrorKind::InvalidArguments, "the session id is empty"))?;
    let on = SessionControlTarget::on(action, session, &target.workspace);
    Ok(match &target.text {
        Some(text) => on.with_text(text),
        None => on,
    })
}

/// What a call of `action` shows: a read for one that watches; for one
/// that acts, the session it acts on and that session's workspace, when
/// known.
fn metadata(action: Action, session: Option<&str>, workspace: Option<&Path>) -> ToolMetadata {
    if action.needs() == Access::Read {
        return ToolMetadata::new().operation(OperationKind::Read);
    }
    let mut metadata = ToolMetadata::new()
        .operation(OperationKind::Other(CONTROL_OPERATION.into()))
        .command_summary(match session {
            Some(session) => format!("{} session {session}", action.name()),
            None => format!("{} session", action.name()),
        });
    if let Some(workspace) = workspace {
        metadata = metadata.affected_path(workspace);
    }
    metadata
}

/// The tool's failure for `error`, whose text is what `rho mcp serve`
/// returns for it too.
fn tool_error(error: ControlError) -> ToolError {
    let kind = match error {
        ControlError::Invalid(_) => ToolErrorKind::InvalidArguments,
        ControlError::Refused(_) | ControlError::UnknownSession(_) | ControlError::Daemon(_) => {
            ToolErrorKind::Execution
        }
    };
    ToolError::new(kind, error.to_string())
}
