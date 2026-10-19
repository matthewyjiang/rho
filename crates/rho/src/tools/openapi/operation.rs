//! The operation index: one lightweight entry per callable operation, and a
//! skip list for the rest.
//!
//! Entries hold names, strings, and [`JsonPointer`]s into the shared document,
//! never cloned schemas. Every `$ref` an entry can reach is checked here, so a
//! broken or external ref skips only the operations that use it.

use std::collections::HashSet;

use serde_json::{Map, Value};
use url::Url;

use super::{
    document::{Document, JsonPointer, RefError},
    params::{self, Params},
    schema::check_refs,
    servers::{self, Servers},
};
use crate::tools::tool_name::{cap_tool_name, sanitize_segment};

/// First segment of every generated tool name.
const TOOL_NAME_PREFIX: &str = "openapi";

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

/// One callable operation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Operation {
    /// Provider-safe and unique within the spec: `openapi__<source>__<id>`.
    pub(crate) tool_name: String,
    /// Where the operation object lives, for reading fields the index skips.
    pub(crate) at: JsonPointer,
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestBody {
    pub(crate) required: bool,
    pub(crate) content: JsonContent,
}

/// A JSON media type entry: `application/json`, a `+json` type, or `*/*`
/// with a schema, which generators such as springdoc emit for JSON.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonContent {
    pub(crate) media_type: String,
    pub(crate) schema: Option<JsonPointer>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SuccessResponse {
    /// A status code such as `200`, or the `2XX` range.
    pub(crate) status: String,
    pub(crate) body: ResponseBody,
}

/// What a successful response carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResponseBody {
    /// No content is declared.
    Empty,
    /// No JSON is offered, but this `text/*` type is.
    Text {
        media_type: String,
    },
    Json(JsonContent),
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
    #[error("declaration is not an object")]
    NotAnObject,
    #[error("WebSocket operations are not supported")]
    WebSocket,
    #[error("SSE operations are not supported")]
    Sse,
    #[error("request body has no JSON content (declared: {})", declared_list(.declared))]
    NonJsonRequestBody { declared: Vec<String> },
    #[error("response type '{media_type}' is neither JSON nor text")]
    UnsupportedResponse { media_type: String },
    #[error("cookie parameter '{name}' is not supported")]
    CookieParam { name: String },
    #[error("parameter '{name}' uses unsupported location '{location}'")]
    UnsupportedParamLocation { name: String, location: String },
    #[error("parameter '{name}' uses unsupported serialization '{style}'")]
    UnsupportedParamStyle { name: String, style: String },
    #[error("invalid parameter declaration: {0}")]
    InvalidParam(&'static str),
    #[error("parameter '{name}' {problem}")]
    MalformedParam { name: String, problem: &'static str },
    #[error("path parameter '{name}' is not declared")]
    UndeclaredPathParam { name: String },
}

fn declared_list(declared: &[String]) -> String {
    if declared.is_empty() {
        "none".to_owned()
    } else {
        declared.join(", ")
    }
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
        // `x-` keys are extensions, not paths.
        for path in paths.keys().filter(|path| !path.starts_with("x-")) {
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
    operation_id: Option<String>,
    at: JsonPointer,
    path_item_at: &'a JsonPointer,
    path_item: &'a Map<String, Value>,
}

impl Indexer<'_> {
    fn path_item(&mut self, path: &str, at: JsonPointer) {
        let resolved = self.document.resolve_at(at);
        let (path_item_at, path_item) = match resolved {
            Ok((at, Value::Object(path_item))) => (at, path_item),
            Ok(_) => return self.skip_path_item(path, SkipReason::NotAnObject),
            Err(error) => return self.skip_path_item(path, error.into()),
        };
        for (key, value) in path_item {
            let Some(method) = HttpMethod::from_key(key) else {
                continue;
            };
            let site = OperationSite {
                method,
                path,
                operation_id: value
                    .get("operationId")
                    .and_then(Value::as_str)
                    .filter(|operation_id| !operation_id.is_empty())
                    .map(str::to_owned),
                at: path_item_at.child(key),
                path_item_at: &path_item_at,
                path_item,
            };
            let result = match value {
                Value::Object(operation) => self.operation(&site, operation),
                _ => Err(SkipReason::NotAnObject),
            };
            match result {
                Ok(operation) => self.operations.push(operation),
                Err(reason) => self.skipped.push(SkippedOperation {
                    method: Some(method),
                    path: path.to_owned(),
                    operation_id: site.operation_id,
                    reason,
                }),
            }
        }
    }

    fn skip_path_item(&mut self, path: &str, reason: SkipReason) {
        self.skipped.push(SkippedOperation {
            method: None,
            path: path.to_owned(),
            operation_id: None,
            reason,
        });
    }

    fn operation(
        &mut self,
        site: &OperationSite<'_>,
        operation: &Map<String, Value>,
    ) -> Result<Operation, SkipReason> {
        if operation.get("x-websocket") == Some(&Value::Bool(true)) {
            return Err(SkipReason::WebSocket);
        }
        let params = params::parse(
            self.document,
            [(site.path_item_at, site.path_item), (&site.at, operation)],
            site.path,
        )?;
        let request_body = self.request_body(site)?;
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
                    .filter_map(|response| match &response.body {
                        ResponseBody::Json(content) => content.schema.clone(),
                        ResponseBody::Empty | ResponseBody::Text { .. } => None,
                    }),
            )
            .collect();
        check_refs(self.document, schemas, &mut self.verified_schemas)?;

        let servers = servers::resolve_declared(operation, self.spec_url)
            .or_else(|| servers::resolve_declared(site.path_item, self.spec_url));
        Ok(Operation {
            tool_name: self.tool_name(site),
            at: site.at.clone(),
            operation_id: site.operation_id.clone(),
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

    fn request_body(&self, site: &OperationSite<'_>) -> Result<Option<RequestBody>, SkipReason> {
        let body_at = site.at.child("requestBody");
        if self.document.get(&body_at).is_none() {
            return Ok(None);
        }
        let (at, body) = self.document.resolve_at(body_at)?;
        let content = self.content(&at.child("content"), body);
        match content.json {
            Some(content) => Ok(Some(RequestBody {
                required: body.get("required") == Some(&Value::Bool(true)),
                content,
            })),
            None => Err(SkipReason::NonJsonRequestBody {
                declared: content.declared,
            }),
        }
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
            let content = self.content(&at.child("content"), response);
            if content.event_stream {
                return Err(SkipReason::Sse);
            }
            // JSON wins over text, and either over other types, since the
            // request can ask for it. opencode instead skips any response that
            // offers a binary type at all, which would drop GitHub's commit
            // and compare operations for their diff alternatives.
            let body = match (content.json, content.text, content.other) {
                (Some(json), _, _) => ResponseBody::Json(json),
                (None, Some(media_type), _) => ResponseBody::Text { media_type },
                (None, None, Some(media_type)) => {
                    return Err(SkipReason::UnsupportedResponse { media_type });
                }
                (None, None, None) => ResponseBody::Empty,
            };
            success.push(SuccessResponse {
                status: status.clone(),
                body,
            });
        }
        Ok(success)
    }

    /// Classify the `content` map of a request body or response, keeping the
    /// first media type of each kind.
    fn content(&self, content_at: &JsonPointer, owner: &Value) -> Content {
        let mut content = Content::default();
        let Some(Value::Object(entries)) = owner.get("content") else {
            return content;
        };
        for (media_type, media) in entries {
            content.declared.push(media_type.clone());
            match self.media_kind(media_type, media) {
                MediaKind::Json => {
                    content.json.get_or_insert_with(|| JsonContent {
                        media_type: media_type.clone(),
                        schema: media
                            .get("schema")
                            .map(|_| content_at.child(media_type).child("schema")),
                    });
                }
                MediaKind::EventStream => content.event_stream = true,
                MediaKind::Text => {
                    content.text.get_or_insert_with(|| media_type.clone());
                }
                MediaKind::Other => {
                    content.other.get_or_insert_with(|| media_type.clone());
                }
            }
        }
        content
    }

    /// JSON and `text/*` types carry text, and so does `*/*`, which
    /// generators such as springdoc put on JSON responses. Everything else,
    /// and any schema with `format: binary`, is neither.
    fn media_kind(&self, media_type: &str, media: &Value) -> MediaKind {
        let essence = media_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if essence == "text/event-stream" {
            return MediaKind::EventStream;
        }
        let json = essence == "application/json" || essence.ends_with("+json");
        if !json && essence != "*/*" && !essence.starts_with("text/") {
            return MediaKind::Other;
        }
        // An unusable ref here is not this check's concern: `check_refs`
        // rejects it if the schema is ever used, and text never is.
        let schema = media.get("schema");
        let resolved = schema.and_then(|schema| match self.document.resolve(schema) {
            Ok(Some((_, target))) => Some(target),
            Ok(None) => Some(schema),
            Err(_) => None,
        });
        let binary = resolved
            .and_then(|schema| schema.get("format"))
            .and_then(Value::as_str)
            == Some("binary");
        match (binary, json || (essence == "*/*" && schema.is_some())) {
            (true, _) => MediaKind::Other,
            (false, true) => MediaKind::Json,
            (false, false) => MediaKind::Text,
        }
    }

    /// A unique tool name from the operation id, or `<method>_<path>` without
    /// one. Collisions after sanitizing or capping get `_2`, `_3`, and so on.
    fn tool_name(&mut self, site: &OperationSite<'_>) -> String {
        let segment = match &site.operation_id {
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

enum MediaKind {
    Json,
    EventStream,
    Text,
    /// Binary, XML, and anything else tools cannot hand back as JSON or text.
    Other,
}

/// A classified `content` map.
#[derive(Default)]
struct Content {
    json: Option<JsonContent>,
    event_stream: bool,
    text: Option<String>,
    other: Option<String>,
    /// Every declared media type, in order.
    declared: Vec<String>,
}

fn is_success_status(status: &str) -> bool {
    let bytes = status.as_bytes();
    bytes.len() == 3
        && bytes[0] == b'2'
        && (bytes[1..].iter().all(u8::is_ascii_digit) || bytes[1..].eq_ignore_ascii_case(b"XX"))
}
