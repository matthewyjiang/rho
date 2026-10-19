//! The operation index: one lightweight entry per callable operation, and a
//! skip list for the rest.
//!
//! Entries hold names, strings, and [`JsonPointer`]s into the shared document,
//! never cloned schemas. Every `$ref` an entry can reach is checked here, so a
//! broken or external ref skips only the operations that use it.

use std::{collections::HashSet, fmt};

use serde_json::{Map, Value};
use url::Url;

use super::{
    document::{Document, JsonPointer, RefError},
    schema::check_refs,
    servers::{self, Servers},
};
use crate::tools::tool_name::{cap_tool_name, sanitize_segment};

/// First segment of every generated tool name.
const TOOL_NAME_PREFIX: &str = "openapi";

/// OpenAPI ignores header parameters with these names; transport sets them.
const IGNORED_HEADER_PARAMS: &[&str] = &["accept", "content-type", "authorization"];

/// An HTTP method that can carry an OpenAPI operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

impl HttpMethod {
    /// The path item key for this method.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Put => "put",
            Self::Post => "post",
            Self::Delete => "delete",
            Self::Options => "options",
            Self::Head => "head",
            Self::Patch => "patch",
            Self::Trace => "trace",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [
            Self::Get,
            Self::Put,
            Self::Post,
            Self::Delete,
            Self::Options,
            Self::Head,
            Self::Patch,
            Self::Trace,
        ]
        .into_iter()
        .find(|method| method.as_str() == key)
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_str().to_ascii_uppercase())
    }
}

/// One callable operation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Operation {
    /// Provider-safe and unique within the spec: `openapi__<source>__<id>`.
    pub(crate) tool_name: String,
    pub(crate) operation_id: Option<String>,
    pub(crate) method: HttpMethod,
    /// The path template, such as `/repos/{owner}/{repo}`.
    pub(crate) path: String,
    pub(crate) summary: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) params: Params,
    pub(crate) request_body: Option<RequestBody>,
    /// 2xx responses in spec order.
    pub(crate) responses: Vec<SuccessResponse>,
    /// Servers declared on the operation or its path item, which replace the
    /// spec's servers. `None` when neither declares any.
    pub(crate) servers: Option<Servers>,
}

/// Parameters grouped by where they are sent.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Params {
    pub(crate) path: Vec<Param>,
    pub(crate) query: Vec<Param>,
    pub(crate) header: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Param {
    pub(crate) name: String,
    /// Always true for path parameters.
    pub(crate) required: bool,
    pub(crate) style: ParamStyle,
    pub(crate) explode: bool,
    pub(crate) schema: JsonPointer,
}

/// The parameter serializations tools support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamStyle {
    /// Path and header parameters.
    Simple,
    /// Query parameters, the default.
    Form,
    /// Query objects as `name[key]=value`.
    DeepObject,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestBody {
    pub(crate) required: bool,
    pub(crate) content: JsonContent,
}

/// A JSON media type entry: `application/json` or a `+json` type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonContent {
    pub(crate) media_type: String,
    pub(crate) schema: Option<JsonPointer>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SuccessResponse {
    /// A status code such as `200`, or the `2XX` range.
    pub(crate) status: String,
    /// `None` when the response has no body or only a non-JSON text body.
    pub(crate) content: Option<JsonContent>,
}

/// An operation left out of the index.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SkippedOperation {
    /// `None` when the whole path item could not be read.
    pub(crate) method: Option<HttpMethod>,
    pub(crate) path: String,
    pub(crate) operation_id: Option<String>,
    pub(crate) reason: SkipReason,
}

/// Why an operation cannot become a tool.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub(crate) enum SkipReason {
    #[error(transparent)]
    Ref(#[from] RefError),
    #[error("operation is not an object")]
    NotAnObject,
    #[error("WebSocket operations are not supported")]
    WebSocket,
    #[error("SSE operations are not supported")]
    Sse,
    #[error("binary request body '{media_type}' is not supported")]
    BinaryRequestBody { media_type: String },
    #[error("request body has no JSON content (declared: {})", .declared.join(", "))]
    NonJsonRequestBody { declared: Vec<String> },
    #[error("binary response '{media_type}' is not supported")]
    BinaryResponse { media_type: String },
    #[error("cookie parameter '{name}' is not supported")]
    CookieParam { name: String },
    #[error("parameter '{name}' uses unsupported location '{location}'")]
    UnsupportedParamLocation { name: String, location: String },
    #[error("parameter '{name}' uses unsupported serialization '{style}'")]
    UnsupportedParamStyle { name: String, style: String },
    #[error("parameter is invalid: {0}")]
    InvalidParam(&'static str),
    #[error("path parameter '{name}' is not declared")]
    UndeclaredPathParam { name: String },
}

/// Index every operation in `document`, naming tools for source `source`.
pub(super) fn index(
    document: &Document,
    source: &str,
    spec_url: &Url,
) -> (Vec<Operation>, Vec<SkippedOperation>) {
    let mut indexer = Indexer {
        document,
        spec_url,
        name_prefix: format!("{TOOL_NAME_PREFIX}__{}__", sanitize_segment(source)),
        used_names: HashSet::new(),
        verified_schemas: HashSet::new(),
        operations: Vec::new(),
        skipped: Vec::new(),
    };
    if let Some(Value::Object(paths)) = document.root().get("paths") {
        let paths_at = JsonPointer::root().child("paths");
        for path in paths.keys() {
            indexer.path_item(path, paths_at.child(path));
        }
    }
    (indexer.operations, indexer.skipped)
}

struct Indexer<'a> {
    document: &'a Document,
    spec_url: &'a Url,
    name_prefix: String,
    used_names: HashSet<String>,
    /// Schemas whose reachable refs already passed [`check_refs`].
    verified_schemas: HashSet<JsonPointer>,
    operations: Vec<Operation>,
    skipped: Vec<SkippedOperation>,
}

/// Where an operation lives, for reading and reporting it.
struct OperationSite<'a> {
    method: HttpMethod,
    path: &'a str,
    at: JsonPointer,
    value: &'a Value,
    path_item: &'a Value,
    path_item_at: &'a JsonPointer,
}

impl Indexer<'_> {
    fn path_item(&mut self, path: &str, at: JsonPointer) {
        let (path_item_at, path_item) = match self.document.resolve_at(at) {
            Ok(resolved) => resolved,
            Err(error) => {
                self.skipped.push(SkippedOperation {
                    method: None,
                    path: path.to_owned(),
                    operation_id: None,
                    reason: error.into(),
                });
                return;
            }
        };
        let Value::Object(entries) = path_item else {
            return;
        };
        for (key, value) in entries {
            let Some(method) = HttpMethod::from_key(key) else {
                continue;
            };
            let site = OperationSite {
                method,
                path,
                at: path_item_at.child(key),
                value,
                path_item,
                path_item_at: &path_item_at,
            };
            let operation_id = value
                .get("operationId")
                .and_then(Value::as_str)
                .filter(|operation_id| !operation_id.is_empty())
                .map(str::to_owned);
            match self.operation(&site, operation_id.clone()) {
                Ok(operation) => self.operations.push(operation),
                Err(reason) => self.skipped.push(SkippedOperation {
                    method: Some(method),
                    path: path.to_owned(),
                    operation_id,
                    reason,
                }),
            }
        }
    }

    fn operation(
        &mut self,
        site: &OperationSite<'_>,
        operation_id: Option<String>,
    ) -> Result<Operation, SkipReason> {
        let Value::Object(operation) = site.value else {
            return Err(SkipReason::NotAnObject);
        };
        if operation.get("x-websocket") == Some(&Value::Bool(true)) {
            return Err(SkipReason::WebSocket);
        }
        let params = self.params(site)?;
        let request_body = self.request_body(site, operation)?;
        let responses = self.responses(site, operation)?;

        let schemas = [&params.path, &params.query, &params.header]
            .into_iter()
            .flatten()
            .map(|param| param.schema.clone())
            .chain(
                request_body
                    .iter()
                    .filter_map(|body| body.content.schema.clone()),
            )
            .chain(
                responses
                    .iter()
                    .filter_map(|response| response.content.as_ref()?.schema.clone()),
            )
            .collect();
        check_refs(self.document, schemas, &mut self.verified_schemas)?;

        let servers = servers::resolve_declared(site.value, self.spec_url)
            .or_else(|| servers::resolve_declared(site.path_item, self.spec_url));
        Ok(Operation {
            tool_name: self.tool_name(operation_id.as_deref(), site),
            operation_id,
            method: site.method,
            path: site.path.to_owned(),
            summary: operation
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_owned),
            tags: operation
                .get("tags")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            params,
            request_body,
            responses,
            servers,
        })
    }

    /// Path item parameters, overridden by operation parameters with the same
    /// name and location, grouped by location in declaration order.
    fn params(&self, site: &OperationSite<'_>) -> Result<Params, SkipReason> {
        let mut declared: Vec<(String, String, JsonPointer, &Map<String, Value>)> = Vec::new();
        for (owner_at, owner) in [(site.path_item_at, site.path_item), (&site.at, site.value)] {
            let Some(Value::Array(list)) = owner.get("parameters") else {
                continue;
            };
            let list_at = owner_at.child("parameters");
            for index in 0..list.len() {
                let (at, param) = self.document.resolve_at(list_at.index(index))?;
                let Value::Object(param) = param else {
                    return Err(SkipReason::InvalidParam("parameter is not an object"));
                };
                let (Some(name), Some(location)) = (
                    param.get("name").and_then(Value::as_str),
                    param.get("in").and_then(Value::as_str),
                ) else {
                    return Err(SkipReason::InvalidParam(
                        "parameter has no name or location",
                    ));
                };
                let entry = (name.to_owned(), location.to_owned(), at, param);
                match declared.iter_mut().find(|(seen_name, seen_location, ..)| {
                    seen_name == name && seen_location == location
                }) {
                    Some(seen) => *seen = entry,
                    None => declared.push(entry),
                }
            }
        }

        let mut params = Params::default();
        for (name, location, at, param) in declared {
            let (group, default_style) = match location.as_str() {
                "path" => (&mut params.path, ParamStyle::Simple),
                "query" => (&mut params.query, ParamStyle::Form),
                "header" if IGNORED_HEADER_PARAMS.contains(&name.to_ascii_lowercase().as_str()) => {
                    continue;
                }
                "header" => (&mut params.header, ParamStyle::Simple),
                "cookie" => return Err(SkipReason::CookieParam { name }),
                _ => return Err(SkipReason::UnsupportedParamLocation { name, location }),
            };
            let unsupported = |style: &str| SkipReason::UnsupportedParamStyle {
                name: name.clone(),
                style: style.to_owned(),
            };
            if param.contains_key("content") {
                return Err(unsupported("content"));
            }
            if !param.contains_key("schema") {
                return Err(SkipReason::InvalidParam(
                    "parameter has neither schema nor content",
                ));
            }
            if param.get("allowReserved") == Some(&Value::Bool(true)) {
                return Err(unsupported("allowReserved"));
            }
            let style = match param.get("style") {
                None => default_style,
                Some(Value::String(style)) => match (default_style, style.as_str()) {
                    (ParamStyle::Simple, "simple") => ParamStyle::Simple,
                    (ParamStyle::Form, "form") => ParamStyle::Form,
                    (ParamStyle::Form, "deepObject") => ParamStyle::DeepObject,
                    _ => return Err(unsupported(style)),
                },
                Some(_) => return Err(SkipReason::InvalidParam("parameter style is not a string")),
            };
            let explode = match param.get("explode") {
                None => style == ParamStyle::Form,
                Some(Value::Bool(explode)) => *explode,
                Some(_) => {
                    return Err(SkipReason::InvalidParam(
                        "parameter explode is not a boolean",
                    ))
                }
            };
            if style == ParamStyle::DeepObject && !explode {
                return Err(unsupported("deepObject without explode"));
            }
            group.push(Param {
                required: location == "path" || param.get("required") == Some(&Value::Bool(true)),
                style,
                explode,
                schema: at.child("schema"),
                name,
            });
        }

        for name in path_template_params(site.path) {
            if !params.path.iter().any(|param| param.name == name) {
                return Err(SkipReason::UndeclaredPathParam {
                    name: name.to_owned(),
                });
            }
        }
        Ok(params)
    }

    fn request_body(
        &self,
        site: &OperationSite<'_>,
        operation: &Map<String, Value>,
    ) -> Result<Option<RequestBody>, SkipReason> {
        if !operation.contains_key("requestBody") {
            return Ok(None);
        }
        let (at, body) = self.document.resolve_at(site.at.child("requestBody"))?;
        let content_at = at.child("content");
        let content = body.get("content").and_then(Value::as_object);
        let entries = content.into_iter().flatten();
        if let Some(content) = json_content(&content_at, entries.clone()) {
            return Ok(Some(RequestBody {
                required: body.get("required") == Some(&Value::Bool(true)),
                content,
            }));
        }
        if let Some((media_type, _)) = entries
            .clone()
            .find(|(media_type, media)| is_binary(media_type, media))
        {
            return Err(SkipReason::BinaryRequestBody {
                media_type: media_type.clone(),
            });
        }
        Err(SkipReason::NonJsonRequestBody {
            declared: entries.map(|(media_type, _)| media_type.clone()).collect(),
        })
    }

    fn responses(
        &self,
        site: &OperationSite<'_>,
        operation: &Map<String, Value>,
    ) -> Result<Vec<SuccessResponse>, SkipReason> {
        let Some(Value::Object(responses)) = operation.get("responses") else {
            return Ok(Vec::new());
        };
        let responses_at = site.at.child("responses");
        let mut success = Vec::new();
        for status in responses.keys().filter(|status| is_success_status(status)) {
            let (at, response) = self.document.resolve_at(responses_at.child(status))?;
            let content_at = at.child("content");
            let entries = response
                .get("content")
                .and_then(Value::as_object)
                .into_iter()
                .flatten();
            if entries
                .clone()
                .any(|(media_type, _)| essence(media_type) == "text/event-stream")
            {
                return Err(SkipReason::Sse);
            }
            let content = json_content(&content_at, entries.clone());
            let binary = entries
                .clone()
                .find(|(media_type, media)| is_binary(media_type, media));
            if let (None, Some((media_type, _))) = (&content, binary) {
                return Err(SkipReason::BinaryResponse {
                    media_type: media_type.clone(),
                });
            }
            success.push(SuccessResponse {
                status: status.clone(),
                content,
            });
        }
        Ok(success)
    }

    /// A unique tool name from the operation id, or `<method>_<path>` without
    /// one. Collisions after sanitizing or capping get `_2`, `_3`, and so on.
    fn tool_name(&mut self, operation_id: Option<&str>, site: &OperationSite<'_>) -> String {
        let segment = match operation_id {
            Some(operation_id) => sanitize_segment(operation_id),
            None => sanitize_segment(&format!("{}_{}", site.method.as_str(), site.path)),
        };
        let mut candidate = cap_tool_name(format!("{}{segment}", self.name_prefix));
        let mut suffix = 2;
        while !self.used_names.insert(candidate.clone()) {
            candidate = cap_tool_name(format!("{}{segment}_{suffix}", self.name_prefix));
            suffix += 1;
        }
        candidate
    }
}

/// The first JSON entry in a `content` map, with a pointer to its schema.
fn json_content<'a>(
    content_at: &JsonPointer,
    mut entries: impl Iterator<Item = (&'a String, &'a Value)>,
) -> Option<JsonContent> {
    let (media_type, media) = entries.find(|(media_type, _)| {
        let essence = essence(media_type);
        essence == "application/json" || essence.ends_with("+json")
    })?;
    Some(JsonContent {
        media_type: media_type.clone(),
        schema: media
            .get("schema")
            .map(|_| content_at.child(media_type).child("schema")),
    })
}

/// Media types that carry bytes rather than text.
fn is_binary(media_type: &str, media: &Value) -> bool {
    let essence = essence(media_type);
    let binary_type = essence == "application/octet-stream"
        || ["image/", "audio/", "video/", "font/"]
            .iter()
            .any(|prefix| essence.starts_with(prefix));
    binary_type || media.pointer("/schema/format").and_then(Value::as_str) == Some("binary")
}

/// A media type without parameters, lowercased: `application/json`.
fn essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_success_status(status: &str) -> bool {
    let bytes = status.as_bytes();
    bytes.len() == 3
        && bytes[0] == b'2'
        && (bytes[1..].iter().all(u8::is_ascii_digit) || bytes[1..].eq_ignore_ascii_case(b"XX"))
}

/// Names of `{param}` segments in a path template.
fn path_template_params(path: &str) -> impl Iterator<Item = &str> {
    path.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
}
