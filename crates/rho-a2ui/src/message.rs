//! The v1.0 wire messages: those an agent sends a renderer, parsed at the
//! boundary, and those a renderer sends back, written to their wire form.

use std::fmt;

use serde_json::{json, Map, Value};

use crate::ident::is_identifier;
use crate::pointer::{Pointer, PointerError};

/// The protocol version this crate speaks, as messages write it.
pub const PROTOCOL_VERSION: &str = "1.0";

/// The message keys of protocol versions before 1.0 that v1.0 renamed.
const PRE_V1_KEYS: [&str; 3] = ["beginRendering", "surfaceUpdate", "dataModelUpdate"];

/// A message from the agent, parsed. Component bodies stay JSON here: which
/// keys are properties, and how to read them, depends on the catalog a
/// component resolves to, which only the surface knows.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentMessage {
    /// `createSurface`.
    CreateSurface(CreateSurface),
    /// `updateComponents`.
    UpdateComponents(UpdateComponents),
    /// `updateDataModel`.
    UpdateDataModel(UpdateDataModel),
    /// `deleteSurface`.
    DeleteSurface {
        /// The surface to delete.
        surface_id: String,
    },
    /// `callRendererFunction`: the agent asks the renderer to run a function.
    CallRendererFunction(CallRendererFunction),
    /// `agentFunctionResponse`: the answer to a call the renderer made.
    AgentFunctionResponse(FunctionResponse),
}

/// `createSurface`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateSurface {
    /// The new surface's id.
    pub surface_id: String,
    /// The default catalog, which v1.0 makes optional.
    pub catalog_id: Option<String>,
    /// Whether every message to the agent carries the surface's data model.
    pub send_data_model: bool,
    /// The initial components, applied after `data_model`.
    pub components: Vec<WireComponent>,
    /// The initial data model.
    pub data_model: Option<Map<String, Value>>,
    /// Surface-level metadata.
    pub metadata: Option<Metadata>,
}

/// `updateComponents`.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateComponents {
    /// The surface to update.
    pub surface_id: String,
    /// The components to add or replace.
    pub components: Vec<WireComponent>,
}

/// `updateDataModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateDataModel {
    /// The surface whose data changes.
    pub surface_id: String,
    /// Where to write; the root when the message names no path.
    pub path: Pointer,
    /// What to write. Null removes what is at `path`.
    pub value: Value,
}

/// `callRendererFunction`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRendererFunction {
    /// The id the answer must carry.
    pub function_call_id: String,
    /// The surface whose data model the call reads, if any.
    pub surface_id: Option<String>,
    /// The call, which names its catalog.
    pub call: Value,
}

/// One entry of a components list: the envelope's `id` and `component`,
/// checked, and every other key as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct WireComponent {
    /// The component's id, a UAX #31 identifier.
    pub id: String,
    /// The component type, a UAX #31 identifier other than `Surface`.
    pub component: String,
    /// Every other key: envelope keys such as `@catalogId`, and properties.
    pub body: Map<String, Value>,
}

/// The `metadata` envelope of a surface, a component or an action: an
/// optional map of extensions under UAX #31 keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    extensions: Option<Map<String, Value>>,
}

/// Why a value is not a `metadata` envelope.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetadataError {
    /// It is not an object holding only `extensions`.
    #[error("metadata must be an object whose only key is extensions")]
    NotAnEnvelope,
    /// `extensions` is not an object.
    #[error("metadata extensions must be an object")]
    NotAnObject,
    /// An extension key is not a UAX #31 identifier.
    #[error("metadata extension key {0:?} is not a UAX #31 identifier")]
    InvalidKey(String),
}

impl Metadata {
    /// Parses a `metadata` envelope.
    ///
    /// # Errors
    ///
    /// Fails when the value is not an object whose only key is `extensions`,
    /// an object whose keys are UAX #31 identifiers.
    pub fn from_json(value: &Value) -> Result<Self, MetadataError> {
        let map = value.as_object().ok_or(MetadataError::NotAnEnvelope)?;
        if map.keys().any(|key| key != "extensions") {
            return Err(MetadataError::NotAnEnvelope);
        }
        let extensions = map
            .get("extensions")
            .map(|extensions| {
                let extensions = extensions.as_object().ok_or(MetadataError::NotAnObject)?;
                match extensions.keys().find(|key| !is_identifier(key)) {
                    Some(key) => Err(MetadataError::InvalidKey(key.clone())),
                    None => Ok(extensions.clone()),
                }
            })
            .transpose()?;
        Ok(Self { extensions })
    }

    /// The extensions, by key.
    #[must_use]
    pub fn extensions(&self) -> Option<&Map<String, Value>> {
        self.extensions.as_ref()
    }

    /// The envelope in its wire form.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match &self.extensions {
            Some(extensions) => json!({ "extensions": extensions }),
            None => json!({}),
        }
    }
}

/// A message from the renderer to the agent.
#[derive(Debug, Clone, PartialEq)]
pub enum RendererMessage {
    /// `action`: the user triggered a component's event.
    Action(ActionMessage),
    /// `error`: something the agent sent, or something it asked for, failed.
    Error(ErrorMessage),
    /// `rendererFunctionResponse`: the answer to a `callRendererFunction`.
    FunctionResponse(FunctionResponse),
}

impl RendererMessage {
    /// The message in its wire form, with its `version`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let (key, body) = match self {
            Self::Action(action) => ("action", action.to_json()),
            Self::Error(error) => ("error", error.to_json()),
            Self::FunctionResponse(response) => ("rendererFunctionResponse", response.to_json()),
        };
        json!({ "version": PROTOCOL_VERSION, key: body })
    }
}

/// `action`.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionMessage {
    /// The event's name.
    pub name: String,
    /// The surface it came from.
    pub surface_id: String,
    /// The component that triggered it.
    pub source_component_id: String,
    /// When, in ISO 8601.
    pub timestamp: String,
    /// The event's context, evaluated.
    pub context: Map<String, Value>,
    /// The event's `userMessage`, evaluated.
    pub user_message: Option<String>,
    /// Metadata the renderer attaches.
    pub metadata: Option<Metadata>,
}

impl ActionMessage {
    fn to_json(&self) -> Value {
        let mut body = Map::new();
        body.insert("name".to_owned(), Value::from(self.name.as_str()));
        body.insert(
            "surfaceId".to_owned(),
            Value::from(self.surface_id.as_str()),
        );
        body.insert(
            "sourceComponentId".to_owned(),
            Value::from(self.source_component_id.as_str()),
        );
        body.insert("timestamp".to_owned(), Value::from(self.timestamp.as_str()));
        body.insert("context".to_owned(), Value::Object(self.context.clone()));
        if let Some(message) = &self.user_message {
            body.insert("userMessage".to_owned(), Value::from(message.as_str()));
        }
        if let Some(metadata) = &self.metadata {
            body.insert("metadata".to_owned(), metadata.to_json());
        }
        Value::Object(body)
    }
}

/// The machine-readable code of an `error` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// A message breaks the protocol's schemas or a catalog's.
    ValidationFailed,
    /// A component is placed under a parent its catalog does not allow.
    UnallowedParent,
    /// A container holds a child its catalog does not allow.
    UnallowedChild,
    /// A message breaks the surface's integrity: an unknown or duplicate
    /// surface, a duplicate id, a missing root, a dangling reference or an
    /// unreachable component.
    IntegrityError,
    /// A cycle, or nesting deeper than the protocol allows.
    RecursionLimitExceeded,
    /// A message names a catalog the renderer lacks, or none where one is
    /// needed.
    CatalogNotFound,
    /// A binding's call names a catalog the surface cannot resolve.
    CatalogError,
    /// A data model write fails.
    DataError,
    /// A binding fails to evaluate.
    ExpressionError,
    /// A local function an action calls fails.
    ExecutionError,
    /// An action is malformed.
    InvalidAction,
    /// A `callRendererFunction` is malformed.
    InvalidFunctionCall,
}

impl ErrorCode {
    /// The code as the wire writes it, such as `VALIDATION_FAILED`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ValidationFailed => "VALIDATION_FAILED",
            Self::UnallowedParent => "UNALLOWED_PARENT",
            Self::UnallowedChild => "UNALLOWED_CHILD",
            Self::IntegrityError => "INTEGRITY_ERROR",
            Self::RecursionLimitExceeded => "RECURSION_LIMIT_EXCEEDED",
            Self::CatalogNotFound => "CATALOG_NOT_FOUND",
            Self::CatalogError => "CATALOG_ERROR",
            Self::DataError => "DATA_ERROR",
            Self::ExpressionError => "EXPRESSION_ERROR",
            Self::ExecutionError => "EXECUTION_ERROR",
            Self::InvalidAction => "INVALID_ACTION",
            Self::InvalidFunctionCall => "INVALID_FUNCTION_CALL",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an `error` message is about: a surface, or a function call the agent
/// made outside any surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorTarget {
    /// The surface, by id. Empty when the message named none.
    Surface(String),
    /// The `functionCallId` of a `callRendererFunction`.
    FunctionCall(String),
}

/// `error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMessage {
    code: ErrorCode,
    target: ErrorTarget,
    path: Option<String>,
    message: String,
}

impl ErrorMessage {
    /// An error about `surface` at `path`, a JSON Pointer into the failing
    /// message's body.
    pub(crate) fn at(
        code: ErrorCode,
        surface: &str,
        path: &str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            target: ErrorTarget::Surface(surface.to_owned()),
            path: Some(path.to_owned()),
            message: message.into(),
        }
    }

    /// An error about `surface` as a whole.
    pub(crate) fn surface(code: ErrorCode, surface: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            target: ErrorTarget::Surface(surface.to_owned()),
            path: None,
            message: message.into(),
        }
    }

    fn function_call(code: ErrorCode, call: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            target: ErrorTarget::FunctionCall(call.to_owned()),
            path: None,
            message: message.into(),
        }
    }

    /// The same error with its path moved below `prefix`.
    pub(crate) fn under(mut self, prefix: &str) -> Self {
        if let Some(path) = &mut self.path {
            path.insert_str(0, prefix);
        }
        self
    }

    /// The code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// What the error is about.
    #[must_use]
    pub fn target(&self) -> &ErrorTarget {
        &self.target
    }

    /// The surface it is about, if any.
    #[must_use]
    pub fn surface_id(&self) -> Option<&str> {
        match &self.target {
            ErrorTarget::Surface(id) => Some(id),
            ErrorTarget::FunctionCall(_) => None,
        }
    }

    /// The JSON Pointer, into the failing message's body, of what failed.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// A short description for a model or a person to read.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    fn to_json(&self) -> Value {
        let mut body = Map::new();
        body.insert("code".to_owned(), Value::from(self.code.as_str()));
        match &self.target {
            ErrorTarget::Surface(id) => {
                body.insert("surfaceId".to_owned(), Value::from(id.as_str()))
            }
            ErrorTarget::FunctionCall(id) => {
                body.insert("functionCallId".to_owned(), Value::from(id.as_str()))
            }
        };
        if let Some(path) = &self.path {
            body.insert("path".to_owned(), Value::from(path.as_str()));
        }
        body.insert("message".to_owned(), Value::from(self.message.as_str()));
        Value::Object(body)
    }
}

impl fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// The answer to a function call: `rendererFunctionResponse` or
/// `agentFunctionResponse`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionResponse {
    /// The id of the call answered.
    pub function_call_id: String,
    /// The value the function returned, or why it failed.
    pub outcome: Result<Value, FunctionFailure>,
}

/// Why a function call failed, as a response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFailure {
    /// A machine-readable code, such as `EXECUTION_ERROR`.
    pub code: String,
    /// A short description.
    pub message: String,
}

impl FunctionResponse {
    fn to_json(&self) -> Value {
        match &self.outcome {
            Ok(value) => json!({ "functionCallId": self.function_call_id, "value": value }),
            Err(failure) => json!({
                "functionCallId": self.function_call_id,
                "error": { "code": failure.code, "message": failure.message },
            }),
        }
    }

    fn from_json(body: &Value) -> Result<Self, String> {
        let map = body.as_object().ok_or("must be an object")?;
        if let Some(key) = map
            .keys()
            .find(|key| !matches!(key.as_str(), "functionCallId" | "value" | "error"))
        {
            return Err(format!("unknown key {key:?}"));
        }
        let function_call_id = map
            .get("functionCallId")
            .and_then(Value::as_str)
            .ok_or("functionCallId must be a string")?
            .to_owned();
        let outcome = match (map.get("value"), map.get("error")) {
            (Some(value), None) => Ok(value.clone()),
            (None, Some(error)) => {
                let text = |key: &str| error.get(key).and_then(Value::as_str).map(str::to_owned);
                match (text("code"), text("message")) {
                    (Some(code), Some(message)) => Err(FunctionFailure { code, message }),
                    _ => return Err("error must have a string code and message".to_owned()),
                }
            }
            _ => return Err("exactly one of value and error is required".to_owned()),
        };
        Ok(Self {
            function_call_id,
            outcome,
        })
    }
}

/// The messages of a payload, each parsed or the `error` that rejects it.
///
/// A payload is one message, a list of them, or a `{"messages": [...]}`
/// wrapper.
#[must_use]
pub fn parse_payload(payload: &Value) -> Vec<Result<AgentMessage, ErrorMessage>> {
    match payload {
        Value::Array(messages) => messages.iter().map(AgentMessage::from_json).collect(),
        Value::Object(map) if map.len() == 1 && map.contains_key("messages") => {
            match &map["messages"] {
                Value::Array(messages) => messages.iter().map(AgentMessage::from_json).collect(),
                _ => vec![Err(ErrorMessage::at(
                    ErrorCode::ValidationFailed,
                    "",
                    "/messages",
                    "messages must be a list",
                ))],
            }
        }
        single => vec![AgentMessage::from_json(single)],
    }
}

impl AgentMessage {
    /// Parses one message.
    ///
    /// # Errors
    ///
    /// Returns the `error` message that rejects it: `VALIDATION_FAILED` for
    /// anything that breaks `agent_to_renderer.json`, with the JSON Pointer of
    /// the offending field in the message's body; `DATA_ERROR` for an
    /// `updateDataModel` path through a forbidden segment; and
    /// `INVALID_FUNCTION_CALL` for a malformed `callRendererFunction`.
    pub fn from_json(message: &Value) -> Result<Self, ErrorMessage> {
        let invalid = |path: &str, text: String| {
            ErrorMessage::at(ErrorCode::ValidationFailed, "", path, text)
        };
        let map = message
            .as_object()
            .ok_or_else(|| invalid("", "a message must be a JSON object".to_owned()))?;
        let actions: Vec<&String> = map.keys().filter(|key| *key != "version").collect();
        let key = match actions.as_slice() {
            [key] => key.as_str(),
            [] => {
                return Err(invalid(
                    "",
                    "a message needs an action such as createSurface".to_owned(),
                ))
            }
            _ => {
                return Err(invalid(
                    "",
                    "Message contains multiple conflicting update actions".to_owned(),
                ))
            }
        };
        let body = &map[key];
        let surface = body
            .get("surfaceId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match map.get("version") {
            None => {
                return Err(ErrorMessage::at(
                    ErrorCode::ValidationFailed,
                    surface,
                    "/version",
                    "Missing required version field",
                ))
            }
            Some(Value::String(version)) if version == PROTOCOL_VERSION => {}
            Some(version) => {
                return Err(ErrorMessage::at(
                    ErrorCode::ValidationFailed,
                    surface,
                    "/version",
                    format!(
                        "Unsupported protocol version '{}'",
                        version.as_str().unwrap_or("?")
                    ),
                ))
            }
        }
        if key == "callRendererFunction" {
            return parse_call_renderer_function(body).map(Self::CallRendererFunction);
        }
        if PRE_V1_KEYS.contains(&key) {
            return Err(invalid(
                "",
                format!("action '{key}' is not supported in protocol version {PROTOCOL_VERSION}"),
            ));
        }
        let mut body = Body::new(body, surface)?;
        let parsed = match key {
            "createSurface" => Self::CreateSurface(body.create_surface()?),
            "updateComponents" => Self::UpdateComponents(UpdateComponents {
                surface_id: body.surface_id()?,
                components: match body.take("components") {
                    Some(list) => components(&list, surface)?,
                    None => return Err(body.invalid("/components", "components is required")),
                },
            }),
            "updateDataModel" => Self::UpdateDataModel(body.update_data_model()?),
            "deleteSurface" => Self::DeleteSurface {
                surface_id: body.surface_id()?,
            },
            "agentFunctionResponse" => {
                let response =
                    FunctionResponse::from_json(&Value::Object(std::mem::take(&mut body.map)))
                        .map_err(|e| invalid("", format!("agentFunctionResponse: {e}")))?;
                Self::AgentFunctionResponse(response)
            }
            other => return Err(invalid("", format!("unknown action '{other}'"))),
        };
        body.finish()?;
        Ok(parsed)
    }
}

/// The body of one message, its fields taken one by one so that a field left
/// over is reported as unknown.
struct Body<'a> {
    map: Map<String, Value>,
    surface: &'a str,
}

impl<'a> Body<'a> {
    fn new(body: &Value, surface: &'a str) -> Result<Self, ErrorMessage> {
        match body {
            Value::Object(map) => Ok(Self {
                map: map.clone(),
                surface,
            }),
            _ => Err(ErrorMessage::at(
                ErrorCode::ValidationFailed,
                surface,
                "",
                "a message body must be an object",
            )),
        }
    }

    fn invalid(&self, path: &str, message: &str) -> ErrorMessage {
        ErrorMessage::at(ErrorCode::ValidationFailed, self.surface, path, message)
    }

    fn take(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key)
    }

    fn surface_id(&mut self) -> Result<String, ErrorMessage> {
        match self.take("surfaceId") {
            Some(Value::String(id)) => Ok(id),
            Some(_) => Err(self.invalid("/surfaceId", "surfaceId must be a string")),
            None => Err(self.invalid("/surfaceId", "surfaceId is required")),
        }
    }

    fn optional_string(&mut self, key: &str) -> Result<Option<String>, ErrorMessage> {
        match self.take(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text)),
            Some(_) => Err(self.invalid(&format!("/{key}"), &format!("{key} must be a string"))),
        }
    }

    fn optional_bool(&mut self, key: &str) -> Result<Option<bool>, ErrorMessage> {
        match self.take(key) {
            None => Ok(None),
            Some(Value::Bool(flag)) => Ok(Some(flag)),
            Some(_) => Err(self.invalid(&format!("/{key}"), &format!("{key} must be a boolean"))),
        }
    }

    fn create_surface(&mut self) -> Result<CreateSurface, ErrorMessage> {
        Ok(CreateSurface {
            surface_id: self.surface_id()?,
            catalog_id: self.optional_string("catalogId")?,
            send_data_model: self.optional_bool("sendDataModel")?.unwrap_or(false),
            components: match self.take("components") {
                Some(list) => components(&list, self.surface)?,
                None => Vec::new(),
            },
            data_model: match self.take("dataModel") {
                None => None,
                Some(Value::Object(model)) => Some(model),
                Some(_) => return Err(self.invalid("/dataModel", "dataModel must be an object")),
            },
            metadata: match self.take("metadata") {
                None => None,
                Some(value) => Some(
                    Metadata::from_json(&value)
                        .map_err(|e| self.invalid("/metadata", &e.to_string()))?,
                ),
            },
        })
    }

    fn update_data_model(&mut self) -> Result<UpdateDataModel, ErrorMessage> {
        let surface_id = self.surface_id()?;
        let path = match self.take("path") {
            None => Pointer::root(),
            Some(Value::String(path)) => Pointer::from_wire(&path).map_err(|e| match e {
                PointerError::ForbiddenSegment { .. } => {
                    ErrorMessage::surface(ErrorCode::DataError, self.surface, e.to_string())
                }
                PointerError::InvalidEscape { .. } | PointerError::NotAbsolute { .. } => {
                    self.invalid("/path", &format!("Invalid path syntax: {e}"))
                }
            })?,
            Some(_) => return Err(self.invalid("/path", "path must be a string")),
        };
        let value = self
            .take("value")
            .ok_or_else(|| self.invalid("/value", "value is required"))?;
        Ok(UpdateDataModel {
            surface_id,
            path,
            value,
        })
    }

    /// Fails on a field no rule took.
    fn finish(self) -> Result<(), ErrorMessage> {
        match self.map.keys().next() {
            Some(key) => Err(self.invalid(&format!("/{key}"), &format!("unknown field {key:?}"))),
            None => Ok(()),
        }
    }
}

/// Parses a components list: the `id` and `component` of each entry.
fn components(list: &Value, surface: &str) -> Result<Vec<WireComponent>, ErrorMessage> {
    let invalid = |path: String, message: String| {
        ErrorMessage::at(ErrorCode::ValidationFailed, surface, &path, message)
    };
    let entries = list.as_array().ok_or_else(|| {
        invalid(
            "/components".to_owned(),
            "components must be a list".to_owned(),
        )
    })?;
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let at = format!("/components/{index}");
            let mut body = entry
                .as_object()
                .ok_or_else(|| invalid(at.clone(), "a component must be an object".to_owned()))?
                .clone();
            let id = match body.remove("id") {
                Some(Value::String(id)) if is_identifier(&id) => id,
                Some(Value::String(id)) => {
                    return Err(invalid(
                        format!("{at}/id"),
                        format!("Component id '{id}' must be a valid UAX #31 identifier"),
                    ))
                }
                _ => {
                    return Err(invalid(
                        format!("{at}/id"),
                        "a component needs a string id".to_owned(),
                    ))
                }
            };
            let component = match body.remove("component") {
                Some(Value::String(name)) if name == "Surface" => {
                    return Err(invalid(
                        format!("{at}/component"),
                        "the Surface component is created by createSurface and cannot be sent"
                            .to_owned(),
                    ))
                }
                Some(Value::String(name)) if is_identifier(&name) => name,
                Some(Value::String(name)) => {
                    return Err(invalid(
                        format!("{at}/component"),
                        format!("component type '{name}' must be a valid UAX #31 identifier"),
                    ))
                }
                _ => {
                    return Err(invalid(
                        format!("{at}/component"),
                        format!("component '{id}' needs a string component type"),
                    ))
                }
            };
            Ok(WireComponent {
                id,
                component,
                body,
            })
        })
        .collect()
}

fn parse_call_renderer_function(body: &Value) -> Result<CallRendererFunction, ErrorMessage> {
    let call_id = body
        .get("functionCallId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let invalid = |message: &str| {
        ErrorMessage::function_call(ErrorCode::InvalidFunctionCall, call_id, message)
    };
    let map = body
        .as_object()
        .ok_or_else(|| invalid("callRendererFunction must be an object"))?;
    if let Some(key) = map.keys().find(|key| {
        !matches!(
            key.as_str(),
            "functionCallId" | "surfaceId" | "callFunction"
        )
    }) {
        return Err(invalid(&format!("unknown field {key:?}")));
    }
    if call_id.is_empty() {
        return Err(invalid("functionCallId must be a non-empty string"));
    }
    let surface_id = match map.get("surfaceId") {
        None => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => return Err(invalid("surfaceId must be a string")),
    };
    let call = map
        .get("callFunction")
        .ok_or_else(|| invalid("callFunction is required"))?;
    let call_map = call
        .as_object()
        .ok_or_else(|| invalid("callFunction must be an object"))?;
    if !call_map.get("@call").is_some_and(Value::is_string) {
        return Err(invalid("callFunction needs a string @call"));
    }
    if call_map.get("args").is_some_and(|args| !args.is_object()) {
        return Err(invalid("callFunction args must be an object"));
    }
    if !call_map.get("catalogId").is_some_and(Value::is_string) {
        return Err(invalid("callFunction needs a string catalogId"));
    }
    Ok(CallRendererFunction {
        function_call_id: call_id.to_owned(),
        surface_id,
        call: call.clone(),
    })
}
