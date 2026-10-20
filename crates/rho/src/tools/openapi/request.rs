//! Tool arguments: the input schema an operation advertises, and the HTTP
//! request a call's arguments build.
//!
//! Path, query, and header parameters are top-level arguments and the JSON
//! body sits under `body`. A name that appears in two locations gets the
//! location as a prefix, as in `path_id` and `query_id`. The model only fills
//! in values: the origin and path come from config and the spec, and path
//! values are percent-encoded, so no argument can redirect the request.

use std::collections::BTreeMap;

use http::{HeaderMap, HeaderName, HeaderValue};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use url::Url;

use super::{
    document::{Document, JsonPointer, RefError},
    operation::{Operation, RequestBody},
    params::{Param, ParamStyle},
    schema::SchemaDirection,
};

/// Everything but RFC 3986 unreserved characters, so a path value can never
/// add a segment, query, or fragment.
const PATH_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The argument that carries the request body.
const BODY_ARG: &str = "body";

/// Providers accept input property names matching `^[a-zA-Z0-9_.-]{1,64}$`.
const MAX_ARG_NAME_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    Path,
    Query,
    Header,
}

impl Location {
    fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
        }
    }
}

#[derive(Debug, Clone)]
struct ParamArg {
    arg: String,
    location: Location,
    param: Param,
    /// What a value read back from a URL converts to.
    kind: ScalarKind,
}

/// The JSON type a parameter's schema declares for a single value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    Integer,
    Number,
    Boolean,
    /// Strings, and anything without one clear scalar type.
    Text,
}

impl ScalarKind {
    /// Read the declared `type`, following one `$ref` into `defs`. A
    /// `["integer", "null"]` union counts as `integer`.
    fn of(schema: &Value, defs: &Map<String, Value>) -> Self {
        let schema = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .and_then(|key| defs.get(key))
            .unwrap_or(schema);
        let declared: Vec<&str> = match schema.get("type") {
            Some(Value::String(kind)) => vec![kind.as_str()],
            Some(Value::Array(kinds)) => kinds
                .iter()
                .filter_map(Value::as_str)
                .filter(|kind| *kind != "null")
                .collect(),
            _ => Vec::new(),
        };
        match declared.as_slice() {
            ["integer"] => Self::Integer,
            ["number"] => Self::Number,
            ["boolean"] => Self::Boolean,
            _ => Self::Text,
        }
    }

    /// `text` as this type, or as a string when it does not parse.
    fn parse(self, text: &str) -> Value {
        let parsed = match self {
            Self::Integer => text.parse::<i64>().ok().map(Value::from),
            Self::Number => text.parse::<serde_json::Number>().ok().map(Value::Number),
            Self::Boolean => text.parse::<bool>().ok().map(Value::Bool),
            Self::Text => None,
        };
        parsed.unwrap_or_else(|| Value::String(text.to_owned()))
    }
}

/// How one operation's arguments map onto a request.
#[derive(Debug, Clone)]
pub(crate) struct RequestShape {
    params: Vec<ParamArg>,
    body: Option<RequestBody>,
}

/// A request built from validated arguments, before any credential.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedRequest {
    pub(crate) url: Url,
    /// Header parameters the arguments supplied.
    pub(crate) headers: HeaderMap,
    /// The JSON body and its media type.
    pub(crate) body: Option<(String, Vec<u8>)>,
}

impl RequestShape {
    /// The shape of `operation` and its tool input schema. Parameter and body
    /// schemas share one root `$defs`, so every `$ref` resolves from the input
    /// schema's root.
    pub(crate) fn new(
        operation: &Operation,
        document: &Document,
    ) -> Result<(Self, Value), RefError> {
        let declared = [
            (Location::Path, &operation.params.path),
            (Location::Query, &operation.params.query),
            (Location::Header, &operation.params.header),
        ]
        .into_iter()
        .flat_map(|(location, params)| params.iter().map(move |param| (location, param)));
        let mut params: Vec<ParamArg> = declared
            .map(|(location, param)| ParamArg {
                arg: arg_name(&param.name),
                location,
                param: param.clone(),
                kind: ScalarKind::Text,
            })
            .collect();

        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for param in &params {
            *counts.entry(param.arg.clone()).or_default() += 1;
        }
        if operation.request_body.is_some() {
            *counts.entry(BODY_ARG.to_owned()).or_default() += 1;
        }
        for param in &mut params {
            if counts[&param.arg] > 1 {
                param.arg = cap(format!("{}_{}", param.location.as_str(), param.arg));
            }
        }
        // Prefixing can still collide, as with `id` in two locations next to
        // a literal `query_id`; numbering settles any remaining clash.
        let mut used: Vec<String> = operation
            .request_body
            .iter()
            .map(|_| BODY_ARG.to_owned())
            .collect();
        for param in &mut params {
            let base = param.arg.clone();
            let mut suffix = 2;
            while used.contains(&param.arg) {
                // Cut the base, not the suffix: a 64-character base would
                // otherwise lose every suffix and never become unique.
                let suffix_text = format!("_{suffix}");
                let mut numbered = base.clone();
                numbered.truncate(MAX_ARG_NAME_LEN - suffix_text.len());
                numbered.push_str(&suffix_text);
                param.arg = numbered;
                suffix += 1;
            }
            used.push(param.arg.clone());
        }
        let mut shape = Self {
            params,
            body: operation.request_body.clone(),
        };
        let schema = shape.input_schema(document)?;
        Ok((shape, schema))
    }

    fn input_schema(&mut self, document: &Document) -> Result<Value, RefError> {
        let body_schema = self
            .body
            .as_ref()
            .and_then(|body| body.content.schema.as_ref());
        let pointers: Vec<&JsonPointer> = self
            .params
            .iter()
            .map(|param| &param.param.schema)
            .chain(body_schema)
            .collect();
        let rendered = document.render_schemas(pointers, SchemaDirection::Request)?;
        let mut roots = rendered.roots.into_iter();
        let mut properties = Map::new();
        let mut required = Vec::new();
        for param in &mut self.params {
            let mut schema = roots.next().unwrap_or_default();
            param.kind = ScalarKind::of(&schema, &rendered.defs);
            if param.arg != param.param.name {
                if let Value::Object(map) = &mut schema {
                    map.entry("description").or_insert_with(|| {
                        Value::String(format!(
                            "Sent as {} parameter `{}`.",
                            param.location.as_str(),
                            param.param.name
                        ))
                    });
                }
            }
            if param.param.required {
                required.push(Value::String(param.arg.clone()));
            }
            properties.insert(param.arg.clone(), schema);
        }
        if let Some(body) = &self.body {
            let schema = match body.content.schema {
                Some(_) => roots.next().unwrap_or_default(),
                None => json!({}),
            };
            if body.required {
                required.push(Value::String(BODY_ARG.to_owned()));
            }
            properties.insert(BODY_ARG.to_owned(), schema);
        }
        let mut schema = Map::from_iter([
            ("type".to_owned(), json!("object")),
            ("properties".to_owned(), Value::Object(properties)),
            ("required".to_owned(), Value::Array(required)),
            ("additionalProperties".to_owned(), Value::Bool(false)),
        ]);
        if !rendered.defs.is_empty() {
            schema.insert("$defs".to_owned(), Value::Object(rendered.defs));
        }
        Ok(Value::Object(schema))
    }

    /// Check `arguments` and build the request against `base`.
    pub(crate) fn build(
        &self,
        base: &Url,
        path_template: &str,
        arguments: &Value,
    ) -> Result<PlannedRequest, String> {
        let Value::Object(arguments) = arguments else {
            return Err("arguments must be a JSON object".to_owned());
        };
        if let Some(unknown) = arguments
            .keys()
            .find(|key| !self.accepts(key))
            .map(String::as_str)
        {
            return Err(format!(
                "unknown argument `{unknown}`; this operation accepts: {}",
                self.argument_names().join(", ")
            ));
        }
        let given = |param: &ParamArg| arguments.get(&param.arg).filter(|value| !value.is_null());
        for param in &self.params {
            if param.param.required && given(param).is_none() {
                return Err(format!("missing required argument `{}`", param.arg));
            }
        }

        let mut path = path_template.to_owned();
        let mut query = Vec::new();
        let mut headers = HeaderMap::new();
        for param in &self.params {
            let Some(value) = given(param) else {
                continue;
            };
            let invalid = |problem: String| format!("argument `{}` {problem}", param.arg);
            match param.location {
                Location::Path => {
                    let encoded = simple_value(value, param.param.explode, encode_path_value)
                        .map_err(invalid)?;
                    if encoded.is_empty() {
                        return Err(invalid("must not be empty".to_owned()));
                    }
                    path = path.replace(&format!("{{{}}}", param.param.name), &encoded);
                }
                Location::Query => {
                    query.extend(query_pairs(&param.param, value).map_err(invalid)?);
                }
                Location::Header => {
                    let value =
                        simple_value(value, param.param.explode, str::to_owned).map_err(invalid)?;
                    let name = HeaderName::try_from(&param.param.name)
                        .map_err(|_| invalid("names an invalid header".to_owned()))?;
                    let value = HeaderValue::try_from(value)
                        .map_err(|_| invalid("is not a valid header value".to_owned()))?;
                    headers.insert(name, value);
                }
            }
        }

        let body = match &self.body {
            None => None,
            Some(declared) => match arguments.get(BODY_ARG).filter(|value| !value.is_null()) {
                None if declared.required => {
                    return Err(format!("missing required argument `{BODY_ARG}`"));
                }
                None => None,
                Some(body) => Some((
                    body_media_type(&declared.content.media_type),
                    serde_json::to_vec(body).map_err(|error| error.to_string())?,
                )),
            },
        };

        // `.` is unreserved, so a value of `.` or `..` survives encoding as a
        // whole segment, and URL parsing would resolve it away.
        if path.split('/').any(|segment| matches!(segment, "." | "..")) {
            return Err("path arguments must not be `.` or `..`".to_owned());
        }
        let mut url = Url::parse(&format!("{}{path}", base.as_str().trim_end_matches('/')))
            .map_err(|error| format!("could not build the request URL: {error}"))?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(&query);
        }
        Ok(PlannedRequest { url, headers, body })
    }

    /// The arguments for the page a `next` link points at: `current` with its
    /// query arguments replaced by the link's. Returns `None` when the link
    /// sets no declared query parameter or would repeat the same call.
    /// Object arguments are kept as they are.
    pub(crate) fn next_arguments(
        &self,
        current: &Value,
        link_query: &[(String, String)],
    ) -> Option<Value> {
        let mut next = current.as_object()?.clone();
        let mut matched = false;
        // Object arguments serialize as `filter[state]=open` or bare keys,
        // which this cannot read back, so they carry over unchanged.
        for param in self.params.iter().filter(|param| {
            param.location == Location::Query
                && !current.get(&param.arg).is_some_and(Value::is_object)
        }) {
            let values: Vec<&String> = link_query
                .iter()
                .filter(|(name, _)| *name == param.param.name)
                .map(|(_, value)| value)
                .collect();
            match values.as_slice() {
                [] => {
                    next.remove(&param.arg);
                }
                [value] => {
                    matched = true;
                    next.insert(param.arg.clone(), param.kind.parse(value));
                }
                values => {
                    matched = true;
                    // Exploded arrays repeat the name; their item type is not
                    // tracked, so items stay strings.
                    next.insert(
                        param.arg.clone(),
                        values.iter().map(|value| json!(value)).collect(),
                    );
                }
            }
        }
        let next = Value::Object(next);
        (matched && &next != current).then_some(next)
    }

    fn accepts(&self, key: &str) -> bool {
        (self.body.is_some() && key == BODY_ARG) || self.params.iter().any(|param| param.arg == key)
    }

    fn argument_names(&self) -> Vec<&str> {
        self.params
            .iter()
            .map(|param| param.arg.as_str())
            .chain(self.body.iter().map(|_| BODY_ARG))
            .collect()
    }
}

/// A provider-safe argument name: invalid characters become `_`.
fn arg_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || "_.-".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        return "param".to_owned();
    }
    cap(name)
}

fn cap(mut name: String) -> String {
    // Names are ASCII here, so any byte index is a character boundary.
    name.truncate(MAX_ARG_NAME_LEN);
    name
}

/// JSON and `*/*` bodies go out as JSON, under their declared type.
fn body_media_type(declared: &str) -> String {
    if declared == "*/*" {
        "application/json".to_owned()
    } else {
        declared.to_owned()
    }
}

fn encode_path_value(value: &str) -> String {
    utf8_percent_encode(value, PATH_VALUE).to_string()
}

/// A scalar as the string a URL or header carries.
fn scalar(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => {
            Err("must be a string, number, or boolean, or a list or object of them".to_owned())
        }
    }
}

/// `simple` style for path and header values: lists as `a,b`, objects as
/// `k,v` or, exploded, `k=v`. `encode` escapes each piece, not the commas.
fn simple_value(
    value: &Value,
    explode: bool,
    encode: impl Fn(&str) -> String,
) -> Result<String, String> {
    Ok(match value {
        Value::Array(items) => items
            .iter()
            .map(|item| scalar(item).map(|item| encode(&item)))
            .collect::<Result<Vec<_>, _>>()?
            .join(","),
        Value::Object(entries) => entries
            .iter()
            .map(|(key, item)| {
                let item = encode(&scalar(item)?);
                let key = encode(key);
                Ok(if explode {
                    format!("{key}={item}")
                } else {
                    format!("{key},{item}")
                })
            })
            .collect::<Result<Vec<_>, String>>()?
            .join(","),
        scalar_value => encode(&scalar(scalar_value)?),
    })
}

/// `form` and `deepObject` query pairs, before URL encoding.
fn query_pairs(param: &Param, value: &Value) -> Result<Vec<(String, String)>, String> {
    let name = &param.name;
    match (param.style, value) {
        (ParamStyle::DeepObject, Value::Object(entries)) => entries
            .iter()
            .map(|(key, item)| Ok((format!("{name}[{key}]"), scalar(item)?)))
            .collect(),
        (ParamStyle::DeepObject, _) => Err("must be an object".to_owned()),
        (_, Value::Array(items)) if param.explode => items
            .iter()
            .map(|item| Ok((name.clone(), scalar(item)?)))
            .collect(),
        (_, Value::Object(entries)) if param.explode => entries
            .iter()
            .map(|(key, item)| Ok((key.clone(), scalar(item)?)))
            .collect(),
        (_, Value::Array(_) | Value::Object(_)) => Ok(vec![(
            name.clone(),
            simple_value(value, false, str::to_owned)?,
        )]),
        (_, value) => Ok(vec![(name.clone(), scalar(value)?)]),
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
