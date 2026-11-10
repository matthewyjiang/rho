//! Actions by name, with JSON in and out, for front ends that expose them
//! as tools: each action's description and input schema, and one call
//! path that parses the arguments, runs the action for its caller, and
//! returns its result as JSON.

use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use super::{
    Action, Caller, CancelParams, ControlError, ListParams, NewParams, PromptParams, ReadParams,
    SessionControl, SteerParams, WaitParams,
};

impl Action {
    /// What the action does, for the model choosing a tool.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::List => {
                "List the rho daemon's sessions, most recently changed first, with each one's \
                 workspace and state (idle, running, waiting_for_input, completed, failed, \
                 cancelled, timed_out, interrupted)."
            }
            Self::Read => {
                "Read a rho session's transcript as text, a window of characters at a time, \
                 its turn in progress included. Pass `next_start` back as `start` to page on."
            }
            Self::New => {
                "Start a rho session in a workspace and send it its first prompt. Returns the \
                 session id at once; call `wait` to follow the turn."
            }
            Self::Prompt => "Start a new turn on a rho session whose last turn ended.",
            Self::Steer => {
                "Send input to a rho session's running turn, which it reads at its next model \
                 request."
            }
            Self::Cancel => "Cancel a rho session's turn in progress.",
            Self::Wait => {
                "Wait, up to `timeout_secs`, for a rho session's turn to end or to stop for a \
                 person's input. Returns its state, how the turn ended with its final answer \
                 when it did, and a `cursor`; while `done` is false, call again with the cursor. \
                 Approvals and questions can only be answered by a person in rho."
            }
        }
    }

    /// The JSON Schema of the action's arguments.
    pub(crate) fn input_schema(self) -> Map<String, Value> {
        match self {
            Self::List => schema::<ListParams>(),
            Self::Read => schema::<ReadParams>(),
            Self::New => schema::<NewParams>(),
            Self::Prompt => schema::<PromptParams>(),
            Self::Steer => schema::<SteerParams>(),
            Self::Cancel => schema::<CancelParams>(),
            Self::Wait => schema::<WaitParams>(),
        }
    }
}

impl SessionControl {
    /// Runs `action` with `arguments` for `caller` and returns its result
    /// as JSON.
    pub(crate) async fn call(
        &self,
        caller: &Caller,
        action: Action,
        arguments: Map<String, Value>,
    ) -> Result<Value, ControlError> {
        match action {
            Action::List => json(self.list(caller, parse(action, arguments)?).await?),
            Action::Read => json(self.read(caller, parse(action, arguments)?).await?),
            Action::New => json(self.new_session(caller, parse(action, arguments)?).await?),
            Action::Prompt => json(self.prompt(caller, parse(action, arguments)?).await?),
            Action::Steer => json(self.steer(caller, parse(action, arguments)?).await?),
            Action::Cancel => json(self.cancel(caller, parse(action, arguments)?).await?),
            Action::Wait => json(self.wait(caller, parse(action, arguments)?).await?),
        }
    }
}

fn schema<T: JsonSchema>() -> Map<String, Value> {
    match serde_json::to_value(schemars::schema_for!(T)) {
        Ok(Value::Object(schema)) => schema,
        _ => unreachable!("a derived schema is a JSON object"),
    }
}

fn parse<T: DeserializeOwned>(
    action: Action,
    arguments: Map<String, Value>,
) -> Result<T, ControlError> {
    serde_json::from_value(Value::Object(arguments)).map_err(|error| {
        ControlError::Invalid(format!("invalid `{}` arguments: {error}", action.name()))
    })
}

fn json(result: impl Serialize) -> Result<Value, ControlError> {
    serde_json::to_value(result)
        .map_err(|error| ControlError::Invalid(format!("could not encode the result: {error}")))
}
