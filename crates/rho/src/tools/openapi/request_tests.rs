use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use url::Url;

use super::super::{OpenApiSpec, SpecInput, DEFAULT_MAX_SPEC_BYTES};
use super::*;

/// The one operation in a spec with `operation` at `GET /items/{id}`.
fn shape(operation: Value) -> (RequestShape, Value) {
    let spec = json!({
        "openapi": "3.1.0",
        "info": {"title": "test", "version": "1"},
        "paths": {"/items/{id}": {"get": operation}},
        "components": {"schemas": {"Filter": {"type": "object", "properties": {"q": {"type": "string"}}}}},
    });
    let bytes = spec.to_string();
    let spec = OpenApiSpec::load(SpecInput {
        name: "test",
        url: &Url::parse("https://api.example.com/spec.json").unwrap(),
        bytes: bytes.as_bytes(),
        max_spec_bytes: DEFAULT_MAX_SPEC_BYTES,
    })
    .unwrap();
    let [operation] = spec.operations.as_slice() else {
        panic!("expected one operation; skipped: {:?}", spec.skipped);
    };
    RequestShape::new(operation, &spec.document).unwrap()
}

fn param(name: &str, location: &str, schema: Value) -> Value {
    json!({"name": name, "in": location, "required": location == "path", "schema": schema})
}

// Covers: a parameter name used in two locations gets location prefixes,
// `body` is reserved for the request body, and every `$ref` resolves from the
// input schema's root `$defs`.
// Owner: OpenAPI request shape (argument naming)
#[test]
fn colliding_names_get_location_prefixes_and_share_defs() {
    let (_, schema) = shape(json!({
        "parameters": [
            param("id", "path", json!({"type": "string"})),
            param("id", "query", json!({"type": "integer"})),
            param("body", "query", json!({"$ref": "#/components/schemas/Filter"})),
            param("X-Trace Id", "header", json!({"type": "string"})),
        ],
        "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Filter"}}}},
        "responses": {"200": {"description": "ok"}},
    }));
    let filter = json!({"$ref": "#/$defs/Filter"});
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "path_id": {"type": "string", "description": "Sent as path parameter `id`."},
                "query_id": {"type": "integer", "description": "Sent as query parameter `id`."},
                "query_body": {"$ref": "#/$defs/Filter", "description": "Sent as query parameter `body`."},
                "X-Trace_Id": {"type": "string", "description": "Sent as header parameter `X-Trace Id`."},
                "body": filter,
            },
            "required": ["path_id"],
            "additionalProperties": false,
            "$defs": {"Filter": {"type": "object", "properties": {"q": {"type": "string"}}}},
        })
    );
}

// Covers: no argument can change the request's origin or path shape. Path
// values are percent-encoded, dot segments are refused, and bad arguments
// fail before a request exists.
// Owner: OpenAPI request builder (URL construction)
#[test]
fn arguments_only_fill_in_values() {
    let (shape, _) = shape(json!({
        "parameters": [
            param("id", "path", json!({"type": "string"})),
            param("tags", "query", json!({"type": "array", "items": {"type": "string"}})),
            json!({"name": "filter", "in": "query", "style": "deepObject", "explode": true, "schema": {"type": "object"}}),
            param("x-mode", "header", json!({"type": "string"})),
        ],
        "responses": {"200": {"description": "ok"}},
    }));
    let base = Url::parse("https://api.example.com/v3/").unwrap();
    let build = |arguments: Value| {
        shape
            .build(&base, "/items/{id}", &arguments)
            .map(|request| {
                (
                    request.url.to_string(),
                    request.headers.get("x-mode").cloned(),
                )
            })
    };
    let ok = |url: &str| Ok((url.to_owned(), None));
    let cases = [
        (
            json!({"id": "a/b?c#d"}),
            ok("https://api.example.com/v3/items/a%2Fb%3Fc%23d"),
        ),
        (
            json!({"id": "%2e%2e"}),
            ok("https://api.example.com/v3/items/%252e%252e"),
        ),
        (
            json!({"id": "@evil.com"}),
            ok("https://api.example.com/v3/items/%40evil.com"),
        ),
        (
            json!({"id": ".."}),
            Err("path arguments must not be `.` or `..`".to_owned()),
        ),
        (
            json!({"id": ""}),
            Err("argument `id` must not be empty".to_owned()),
        ),
        (json!({}), Err("missing required argument `id`".to_owned())),
        (
            json!({"id": "1", "url": "https://evil.com"}),
            Err(
                "unknown argument `url`; this operation accepts: id, tags, filter, x-mode"
                    .to_owned(),
            ),
        ),
        (
            json!({"id": "1", "tags": ["a", "b c"], "filter": {"state": "open"}}),
            ok("https://api.example.com/v3/items/1?tags=a&tags=b+c&filter%5Bstate%5D=open"),
        ),
        (
            json!({"id": "1", "x-mode": "fast"}),
            Ok((
                "https://api.example.com/v3/items/1".to_owned(),
                Some(HeaderValue::from_static("fast")),
            )),
        ),
        (
            json!({"id": "1", "x-mode": "a\nb"}),
            Err("argument `x-mode` is not a valid header value".to_owned()),
        ),
    ];
    for (arguments, expected) in cases {
        assert_eq!(build(arguments.clone()), expected, "{arguments}");
    }
}

// Covers: `next` is the current call with its query arguments replaced by the
// link's, typed as each parameter's schema declares, or null when the link
// sets nothing this operation declares.
// Owner: OpenAPI pagination
#[test]
fn next_arguments_replace_query_arguments_from_the_link() {
    let (shape, _) = shape(json!({
        "parameters": [
            param("id", "path", json!({"type": "string"})),
            param("page", "query", json!({"type": "integer"})),
            param("cursor", "query", json!({"type": "string"})),
            param("draft", "query", json!({"type": ["boolean", "null"]})),
        ],
        "responses": {"200": {"description": "ok"}},
    }));
    let pairs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    };
    let cases = [
        (
            json!({"id": "1", "page": 1}),
            pairs(&[("page", "2")]),
            Some(json!({"id": "1", "page": 2})),
        ),
        (
            json!({"id": "1", "cursor": "abc"}),
            pairs(&[("cursor", "def"), ("since", "x")]),
            Some(json!({"id": "1", "cursor": "def"})),
        ),
        (
            json!({"id": "1", "page": 3}),
            pairs(&[("cursor", "def")]),
            Some(json!({"id": "1", "cursor": "def"})),
        ),
        (
            json!({"id": "1"}),
            pairs(&[("page", "2"), ("draft", "true"), ("cursor", "7")]),
            Some(json!({"id": "1", "page": 2, "draft": true, "cursor": "7"})),
        ),
        (
            json!({"id": "1"}),
            pairs(&[("page", "last")]),
            Some(json!({"id": "1", "page": "last"})),
        ),
        (json!({"id": "1"}), pairs(&[("since", "x")]), None),
        (json!({"id": "1", "page": 2}), pairs(&[("page", "2")]), None),
    ];
    for (current, link, expected) in cases {
        assert_eq!(
            shape.next_arguments(&current, &link),
            expected,
            "{current} {link:?}"
        );
    }
}
