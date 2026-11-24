//! Actions by name, with JSON in and out, for front ends that expose them
//! as tools: each action's description and input schema, and one call
//! path that parses the arguments, runs the action for its caller, and
//! returns its result as JSON.

use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use super::{
    Action, AnswerParams, Caller, CancelParams, ControlError, ListParams, NewParams, PromptParams,
    ReadParams, SessionControl, SteerParams, WaitParams,
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
                "Wait, up to `timeout_secs`, for a rho session to settle, with no turn running \
                 and none rho will start for its background work, or to stop for a person's \
                 input, a subagent's question included. Returns its state, how its last turn \
                 ended with its final answer when it did, and a `cursor`; while `done` is false, \
                 call again with the cursor. Approvals and plan approvals can only be answered \
                 by a person in rho; a question only by a person or the session that started \
                 the one asking."
            }
            Self::Answer => {
                "Answer a question that a session you started asks, as `wait`'s `inputs` show \
                 it: its `id` as `request`, and `answers` by question id, or `decline: true`. \
                 Only questions: approvals wait for a person. Answer only what your \
                 instructions already settle; leave anything else for the person."
            }
        }
    }

    /// The JSON Schema of the action's arguments.
    pub(crate) fn input_schema(self) -> Map<String, Value> {
        match self {
            Self::List => schema_of::<ListParams>(),
            Self::Read => schema_of::<ReadParams>(),
            Self::New => schema_of::<NewParams>(),
            Self::Prompt => schema_of::<PromptParams>(),
            Self::Steer => schema_of::<SteerParams>(),
            Self::Cancel => schema_of::<CancelParams>(),
            Self::Wait => schema_of::<WaitParams>(),
            Self::Answer => schema_of::<AnswerParams>(),
        }
    }
}

impl SessionControl {
    /// Checks and runs `action` with `arguments` for `caller` and returns
    /// its result as JSON.
    pub(crate) async fn call(
        &self,
        caller: &Caller,
        action: Action,
        arguments: Map<String, Value>,
    ) -> Result<Value, ControlError> {
        let checked = self.check(caller, action, arguments).await?;
        self.run(caller, checked).await
    }
}

/// The JSON Schema of `T`, as a tool's input schema.
pub(crate) fn schema_of<T: JsonSchema>() -> Map<String, Value> {
    match serde_json::to_value(schemars::schema_for!(T)) {
        Ok(Value::Object(schema)) => schema,
        _ => unreachable!("a derived schema is a JSON object"),
    }
}

/// Tool `tool`'s `arguments` as its params `T`.
pub(crate) fn parse_arguments<T: DeserializeOwned>(
    tool: &str,
    arguments: Map<String, Value>,
) -> Result<T, ControlError> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|error| ControlError::Invalid(format!("invalid `{tool}` arguments: {error}")))
}

pub(super) fn json(result: impl Serialize) -> Result<Value, ControlError> {
    serde_json::to_value(result)
        .map_err(|error| ControlError::Invalid(format!("could not encode the result: {error}")))
}
