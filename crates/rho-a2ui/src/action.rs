//! A component's `Action`, an event for the agent or a function to run
//! locally, and dispatching it when the user interacts with the component.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Timelike};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::coerce::to_string;
use crate::data_context::{DataContext, UserActivation};
use crate::expr::{Call, Expr, ExpressionError};
use crate::functions::Effect;
use crate::message::{ActionMessage, ErrorCode, ErrorMessage, Metadata, RendererMessage};
use crate::pointer::Pointer;
use crate::processor::{data_models, Change, MessageProcessor};
use crate::resolve::{evaluation_error, Scope};

/// What a component does when the user interacts with it.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send an `action` message to the agent.
    Event(Event),
    /// Run a function on the renderer.
    Call(Call),
}

/// An event action: the `action` message to send, unevaluated.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// The event's name.
    pub name: String,
    /// The context, an object whose members may be dynamic.
    pub context: Expr,
    /// A `DynamicString` describing what the user did.
    pub user_message: Option<Expr>,
}

/// Why a value is not an `Action`.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ActionError {
    /// It is not an object holding exactly one of `event` and
    /// `functionCall`.
    #[error("an action must be an object with exactly one of event and functionCall")]
    NotAnAction,
    /// The event has no string `name`.
    #[error("an event action needs a string name")]
    MissingName,
    /// The event has a key other than `name`, `context` and `userMessage`.
    #[error("an event action has unknown key {0:?}")]
    UnknownKey(String),
    /// The event's `context` is not an object.
    #[error("an event action's context must be an object")]
    ContextNotAnObject,
    /// The event's `userMessage` is neither a string, a binding nor a call.
    #[error("an event action's userMessage must be a string, a binding or a call")]
    InvalidUserMessage,
    /// `functionCall` is not a function call.
    #[error("functionCall must be a function call")]
    NotACall,
    /// A dynamic value in it is malformed.
    #[error(transparent)]
    Expression(#[from] ExpressionError),
}

impl Action {
    /// Parses an action in its wire form.
    ///
    /// # Errors
    ///
    /// Fails when the value is not an `Action` of `common_types.json`, or
    /// holds a malformed binding or call.
    pub fn from_json(value: &Value) -> Result<Self, ActionError> {
        let map = value.as_object().ok_or(ActionError::NotAnAction)?;
        match (map.get("event"), map.get("functionCall"), map.len()) {
            (Some(event), None, 1) => Ok(Self::Event(parse_event(event)?)),
            (None, Some(call), 1) => match Expr::from_json(call)? {
                Expr::Call(call) => Ok(Self::Call(call)),
                _ => Err(ActionError::NotACall),
            },
            _ => Err(ActionError::NotAnAction),
        }
    }
}

fn parse_event(event: &Value) -> Result<Event, ActionError> {
    let map = event.as_object().ok_or(ActionError::NotAnAction)?;
    if let Some(key) = map
        .keys()
        .find(|key| !matches!(key.as_str(), "name" | "context" | "userMessage"))
    {
        return Err(ActionError::UnknownKey(key.clone()));
    }
    let name = map
        .get("name")
        .and_then(Value::as_str)
        .ok_or(ActionError::MissingName)?
        .to_owned();
    let context = match map.get("context") {
        None => Expr::Literal(Value::Object(Map::new())),
        Some(context @ Value::Object(_)) => Expr::from_json(context)?,
        Some(_) => return Err(ActionError::ContextNotAnObject),
    };
    let user_message = map
        .get("userMessage")
        .map(|message| match Expr::from_json(message)? {
            Expr::Literal(Value::String(text)) => Ok(Expr::Literal(Value::String(text))),
            dynamic @ (Expr::Path(_) | Expr::Call(_)) => Ok(dynamic),
            _ => Err(ActionError::InvalidUserMessage),
        })
        .transpose()?;
    Ok(Event {
        name,
        context,
        user_message,
    })
}

/// A user's interaction with a component that has an action.
#[derive(Debug, Clone, PartialEq)]
pub struct Interaction<'a> {
    /// The surface.
    pub surface_id: &'a str,
    /// The component the user interacted with.
    pub source_component_id: &'a str,
    /// The scope the component was resolved in.
    pub scope: Scope,
    /// Whether a gesture of the user's triggered it, which a function such
    /// as `openUrl` requires.
    pub activation: UserActivation,
    /// Metadata to attach to the `action` message.
    pub metadata: Option<Metadata>,
    /// When it happened.
    pub at: SystemTime,
}

/// What dispatching an action did.
#[derive(Debug, Clone, Default, PartialEq)]
#[must_use]
pub struct Dispatched {
    /// What the action changed in the surface's data model.
    pub changes: Vec<Change>,
    /// The messages to send the agent: the `action`, or the `error` that
    /// stopped it.
    pub outbound: Vec<RendererMessage>,
    /// What the host should do, such as open a URL.
    pub effects: Vec<Effect>,
    /// The surface's data model, for the transport to attach to the
    /// `action`, when the surface was created with `sendDataModel`.
    pub data_model: Option<Value>,
}

impl MessageProcessor {
    /// Runs `action` for `interaction`: evaluates an event's context and
    /// `userMessage` into an `action` message, or runs a function call,
    /// applying the data writes it asks for. Returns `None` when there is no
    /// such surface.
    pub fn dispatch_action(
        &mut self,
        interaction: &Interaction<'_>,
        action: &Action,
    ) -> Option<Dispatched> {
        let sid = interaction.surface_id;
        let surface = self.surfaces.get(sid)?;
        let mut context = self.context(surface, &interaction.scope, interaction.activation);
        let mut dispatched = Dispatched::default();
        match action {
            Action::Event(event) => match event_message(&mut context, event, interaction) {
                Ok(message) => {
                    dispatched.outbound.push(RendererMessage::Action(message));
                    dispatched.data_model =
                        data_models(std::iter::once(surface).filter(|s| s.send_data_model));
                }
                Err(error) => dispatched.outbound.push(RendererMessage::Error(error)),
            },
            Action::Call(call) => match context.evaluate(&Expr::Call(call.clone())) {
                Ok(_) => dispatched.effects = context.take_effects(),
                Err(error) => {
                    dispatched
                        .outbound
                        .push(RendererMessage::Error(ErrorMessage::surface(
                            ErrorCode::ExecutionError,
                            sid,
                            format!("function call action '{}' failed: {error}", call.name),
                        )));
                }
            },
        }
        let writes: Vec<(Pointer, Value)> = dispatched
            .effects
            .extract_if(.., |effect| matches!(effect, Effect::SetData { .. }))
            .filter_map(|effect| match effect {
                Effect::SetData { path, value } => Some((path, value)),
                Effect::OpenUrl(_) => None,
            })
            .collect();
        let surface = self.surfaces.get_mut(sid)?;
        for (path, value) in writes {
            match surface.data.set(&path, value) {
                Ok(fired) => dispatched.changes.push(Change::DataUpdated {
                    surface: sid.to_owned(),
                    path,
                    fired,
                }),
                Err(error) => {
                    dispatched
                        .outbound
                        .push(RendererMessage::Error(ErrorMessage::surface(
                            ErrorCode::ExecutionError,
                            sid,
                            format!("an action's data write failed: {error}"),
                        )));
                }
            }
        }
        Some(dispatched)
    }
}

/// The `action` message an event action sends, with its context and
/// `userMessage` evaluated.
fn event_message(
    context: &mut DataContext<'_>,
    event: &Event,
    interaction: &Interaction<'_>,
) -> Result<ActionMessage, ErrorMessage> {
    let sid = interaction.surface_id;
    let what = format!("action '{}'", event.name);
    let user_message = event
        .user_message
        .as_ref()
        .map(|message| context.evaluate(message))
        .transpose()
        .map_err(|error| evaluation_error(sid, &what, &error))?;
    let Value::Object(evaluated) = context
        .evaluate(&event.context)
        .map_err(|error| evaluation_error(sid, &what, &error))?
    else {
        return Err(ErrorMessage::surface(
            ErrorCode::InvalidAction,
            sid,
            format!("the context of {what} is not an object"),
        ));
    };
    Ok(ActionMessage {
        name: event.name.clone(),
        surface_id: sid.to_owned(),
        source_component_id: interaction.source_component_id.to_owned(),
        timestamp: timestamp(interaction.at),
        context: evaluated,
        user_message: user_message.as_ref().map(to_string),
        metadata: interaction.metadata.clone(),
    })
}

/// `at` in ISO 8601, to the millisecond, in UTC.
fn timestamp(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = i64::try_from(since.as_secs()).unwrap_or(i64::MAX);
    let Some(time) = DateTime::from_timestamp(seconds, since.subsec_nanos()) else {
        return "1970-01-01T00:00:00.000Z".to_owned();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        time.year(),
        time.month(),
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        since.subsec_millis(),
    )
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
