use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{
    document::{JsonPointer, RefError},
    operation::{
        HttpMethod::{Delete, Get, Post, Put},
        JsonContent, Param, ParamStyle, Params, RequestBody, SkipReason, SuccessResponse,
    },
    schema::SchemaDirection,
    servers::{resolve_spec, ServerUrlError},
    *,
};

const SLOPPY: &[u8] = include_bytes!("openapi/fixtures/sloppy.yaml");
/// Five operations of GitHub's REST spec with their components, minus
/// descriptions and examples.
const GITHUB_SUBSET: &[u8] = include_bytes!("openapi/fixtures/github_subset.json");

fn url(raw: &str) -> Url {
    Url::parse(raw).expect("valid URL")
}

fn load(name: &str, bytes: &[u8]) -> OpenApiSpec {
    OpenApiSpec::load(SpecInput {
        name,
        url: &url("https://example.com/specs/api.yaml"),
        bytes,
        max_spec_bytes: DEFAULT_MAX_SPEC_BYTES,
    })
    .expect("spec loads")
}

fn pointer(reference: &str) -> JsonPointer {
    JsonPointer::from_ref(reference).expect("local ref")
}

// Covers: each sloppy construct skips only its own operation with a reason,
// while names stay unique and provider-safe.
// Owner: OpenAPI operation index.
#[test]
fn sloppy_spec_skips_only_unusable_operations() {
    let spec = load("sloppy", SLOPPY);

    let indexed: Vec<_> = spec
        .operations
        .iter()
        .map(|operation| {
            (
                operation.tool_name.as_str(),
                operation.method,
                operation.path.as_str(),
            )
        })
        .collect();
    assert_eq!(
        indexed,
        vec![
            ("openapi__sloppy__pets_list", Get, "/pets"),
            // `pets-list` sanitizes to the same segment as `pets/list`.
            ("openapi__sloppy__pets_list_2", Post, "/pets"),
            // No operationId: named from the method and path.
            ("openapi__sloppy__get_pets_id", Get, "/pets/{id}"),
            // A recursive schema is legal.
            ("openapi__sloppy__tree_get", Get, "/tree"),
            (
                "openapi__sloppy__aVeryLongOperationIdentifierThatKeepsG-dcffd71c",
                Get,
                "/long"
            ),
        ]
    );

    // The relative, templated spec server resolves against the spec URL, and
    // an operation's own servers replace it.
    assert_eq!(spec.servers, vec![Ok(url("https://example.com/v2"))]);
    assert_eq!(
        spec.operations[3].servers,
        Some(vec![Ok(url("https://trees.example.com"))])
    );

    let skipped: Vec<_> = spec
        .skipped
        .iter()
        .map(|skipped| {
            (
                skipped.method,
                skipped.path.as_str(),
                skipped.reason.clone(),
            )
        })
        .collect();
    let name = |name: &str| name.to_owned();
    assert_eq!(
        skipped,
        vec![
            (
                Some(Delete),
                "/pets/{id}",
                SkipReason::CookieParam {
                    name: name("session")
                }
            ),
            (Some(Get), "/pets/{id}/events", SkipReason::Sse),
            (Some(Get), "/pets/{id}/live", SkipReason::WebSocket),
            (
                Some(Get),
                "/broken",
                RefError::Broken(name("#/components/schemas/Missing")).into()
            ),
            (
                Some(Get),
                "/external",
                RefError::External(name("common.yaml#/components/parameters/page")).into()
            ),
            (
                Some(Get),
                "/cycle",
                RefError::Cycle(name("#/components/parameters/loopA")).into()
            ),
            (
                Some(Put),
                "/upload",
                SkipReason::BinaryRequestBody {
                    media_type: name("application/octet-stream")
                }
            ),
            (
                Some(Post),
                "/form",
                SkipReason::NonJsonRequestBody {
                    declared: vec![name("application/x-www-form-urlencoded")]
                }
            ),
            (
                Some(Get),
                "/matrix/{id}",
                SkipReason::UnsupportedParamStyle {
                    name: name("id"),
                    style: name("matrix")
                }
            ),
            (
                Some(Get),
                "/undeclared/{id}",
                SkipReason::UndeclaredPathParam { name: name("id") }
            ),
        ]
    );
}

// Covers: a real spec's shared parameters, JSON body, and response schema are
// indexed as pointers, and its unusable operations are skipped.
// Owner: OpenAPI operation index.
#[test]
fn github_subset_indexes_params_bodies_and_responses() {
    let spec = load("github", GITHUB_SUBSET);

    let names: Vec<_> = spec
        .operations
        .iter()
        .map(|operation| operation.tool_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "openapi__github__meta_root",
            "openapi__github__issues_create_comment",
            "openapi__github__repos_list_tags",
        ]
    );
    let skipped: Vec<_> = spec
        .skipped
        .iter()
        .map(|skipped| (skipped.operation_id.as_deref(), skipped.reason.clone()))
        .collect();
    assert_eq!(
        skipped,
        vec![
            (
                Some("markdown/render-raw"),
                SkipReason::NonJsonRequestBody {
                    declared: vec!["text/plain".to_owned(), "text/x-markdown".to_owned()]
                }
            ),
            (
                Some("repos/upload-release-asset"),
                SkipReason::BinaryRequestBody {
                    media_type: "application/octet-stream".to_owned()
                }
            ),
        ]
    );

    let path_param = |name: &str, component: &str| Param {
        name: name.to_owned(),
        required: true,
        style: ParamStyle::Simple,
        explode: false,
        schema: pointer(&format!("#/components/parameters/{component}/schema")),
    };
    let comments = "/paths/~1repos~1{owner}~1{repo}~1issues~1{issue_number}~1comments/post";
    assert_eq!(
        spec.operations[1],
        Operation {
            tool_name: "openapi__github__issues_create_comment".to_owned(),
            operation_id: Some("issues/create-comment".to_owned()),
            method: Post,
            path: "/repos/{owner}/{repo}/issues/{issue_number}/comments".to_owned(),
            summary: Some("Create an issue comment".to_owned()),
            tags: vec!["issues".to_owned()],
            params: Params {
                path: vec![
                    path_param("owner", "owner"),
                    path_param("repo", "repo"),
                    path_param("issue_number", "issue-number"),
                ],
                query: Vec::new(),
                header: Vec::new(),
            },
            request_body: Some(RequestBody {
                required: true,
                content: JsonContent {
                    media_type: "application/json".to_owned(),
                    schema: Some(pointer(&format!(
                        "#{comments}/requestBody/content/application~1json/schema"
                    ))),
                },
            }),
            responses: vec![SuccessResponse {
                status: "201".to_owned(),
                content: Some(JsonContent {
                    media_type: "application/json".to_owned(),
                    schema: Some(pointer(&format!(
                        "#{comments}/responses/201/content/application~1json/schema"
                    ))),
                }),
            }],
            servers: None,
        }
    );
}

// Covers: rendered schemas are self-contained 2020-12 with refs moved into
// `$defs`, recursion stays finite, and each direction hides its properties.
// Owner: OpenAPI schema renderer.
#[test]
fn rendered_schemas_hide_properties_by_direction() {
    let spec = load("sloppy", SLOPPY);
    let schema_of = |tool_name: &str, direction: SchemaDirection| {
        let operation = spec
            .operations
            .iter()
            .find(|operation| operation.tool_name == tool_name)
            .expect("indexed operation");
        let content = match direction {
            SchemaDirection::Request => operation.request_body.as_ref().map(|body| &body.content),
            SchemaDirection::Response => operation.responses[0].content.as_ref(),
        };
        let at = content.and_then(|content| content.schema.as_ref());
        spec.document
            .render_schema(at.expect("schema"), direction)
            .expect("renders")
    };
    let pet = |required: Value, id: Option<Value>, secret: Option<Value>| {
        let mut properties = serde_json::Map::new();
        properties.extend(id.map(|id| ("id".to_owned(), id)));
        properties.insert("name".to_owned(), json!({"type": "string"}));
        properties.insert("tag".to_owned(), json!({"type": ["string", "null"]}));
        properties.extend(secret.map(|secret| ("secret".to_owned(), secret)));
        properties.insert(
            "weight".to_owned(),
            json!({"type": "number", "exclusiveMinimum": 0}),
        );
        json!({"type": "object", "required": required, "properties": properties})
    };

    assert_eq!(
        schema_of("openapi__sloppy__pets_list_2", SchemaDirection::Request),
        json!({
            "$ref": "#/$defs/Pet",
            "$defs": {"Pet": pet(
                json!(["name", "secret"]),
                None,
                Some(json!({"type": "string", "writeOnly": true})),
            )},
        })
    );
    assert_eq!(
        schema_of("openapi__sloppy__pets_list", SchemaDirection::Response),
        json!({
            "type": "array",
            "items": {"$ref": "#/$defs/Pet"},
            "$defs": {"Pet": pet(
                json!(["id", "name"]),
                Some(json!({"type": "integer", "readOnly": true})),
                None,
            )},
        })
    );
    let node = json!({
        "type": "object",
        "properties": {"children": {"type": "array", "items": {"$ref": "#/$defs/Node"}}},
    });
    assert_eq!(
        schema_of("openapi__sloppy__tree_get", SchemaDirection::Response),
        json!({"$ref": "#/$defs/Node", "$defs": {"Node": node}})
    );
}

// Covers: OpenAPI 3.0 keywords become their 2020-12 form, and 3.1 schemas
// pass through with `$ref` siblings kept.
// Owner: OpenAPI schema renderer.
#[test]
fn render_normalizes_3_0_schemas_to_2020_12() {
    let target_defs = json!({"t": {"type": "string"}});
    let cases = [
        (
            "3.0.3",
            json!({"type": "string", "nullable": true}),
            json!({"type": ["string", "null"]}),
        ),
        (
            "3.0.3",
            json!({"type": "string", "enum": ["a"], "nullable": true}),
            json!({"type": ["string", "null"], "enum": ["a", null]}),
        ),
        (
            "3.0.3",
            json!({"oneOf": [{"type": "string"}, {"type": "integer"}], "nullable": true}),
            json!({"anyOf": [
                {"oneOf": [{"type": "string"}, {"type": "integer"}]},
                {"type": "null"},
            ]}),
        ),
        (
            "3.0.3",
            json!({"minimum": 1, "exclusiveMinimum": true, "maximum": 9, "exclusiveMaximum": false}),
            json!({"exclusiveMinimum": 1, "maximum": 9}),
        ),
        (
            "3.0.3",
            json!({"$ref": "#/components/schemas/t", "description": "ignored in 3.0"}),
            json!({"$ref": "#/$defs/t", "$defs": target_defs}),
        ),
        (
            "3.1.0",
            json!({"type": ["string", "null"], "exclusiveMinimum": 1}),
            json!({"type": ["string", "null"], "exclusiveMinimum": 1}),
        ),
        (
            "3.1.0",
            json!({"$ref": "#/components/schemas/t", "description": "kept in 3.1"}),
            json!({"$ref": "#/$defs/t", "description": "kept in 3.1", "$defs": target_defs}),
        ),
    ];
    for (version, schema, expected) in cases {
        let spec = json!({
            "openapi": version,
            "paths": {},
            "components": {"schemas": {"s": schema, "t": {"type": "string"}}},
        });
        let document = Document::parse(spec.to_string().as_bytes()).expect("parses");
        let rendered = document
            .render_schema(&pointer("#/components/schemas/s"), SchemaDirection::Request)
            .expect("renders");
        assert_eq!((version, &schema, rendered), (version, &schema, expected));
    }
}

// Covers: only an oversized, unparsable, or non-3.x document fails the whole
// source, and the size error names the limit and the actual size.
// Owner: OpenAPI spec loader.
#[test]
fn load_fails_whole_source_only_for_document_problems() {
    type IsExpected = fn(&LoadError) -> bool;
    let cases: [(&[u8], usize, IsExpected); 6] = [
        (b"{\"openapi\": \"3.1.0\"}", 8, |error| {
            matches!(error, LoadError::TooLarge { size: 20, limit: 8 })
        }),
        (b"{\"openapi\": ", 64, |error| {
            matches!(error, LoadError::Json(_))
        }),
        (b"openapi: [3.1", 64, |error| {
            matches!(error, LoadError::Yaml(_))
        }),
        (b"swagger: \"2.0\"\npaths: {}\n", 64, |error| {
            matches!(error, LoadError::Swagger2)
        }),
        (
            b"openapi: 3.2.0\n",
            64,
            |error| matches!(error, LoadError::UnsupportedVersion(version) if version == "3.2.0"),
        ),
        (b"info: {}\n", 64, |error| {
            matches!(error, LoadError::MissingVersion)
        }),
    ];
    for (bytes, max_spec_bytes, expected) in cases {
        let error = OpenApiSpec::load(SpecInput {
            name: "api",
            url: &url("https://example.com/api.yaml"),
            bytes,
            max_spec_bytes,
        })
        .expect_err("load fails");
        assert!(
            expected(&error),
            "{}: {error:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

// Covers: relative and templated server URLs resolve to absolute HTTP(S) URLs,
// and unusable entries carry a reason instead of failing the spec.
// Owner: OpenAPI server resolution.
#[test]
fn servers_resolve_against_the_spec_url() {
    let https_spec = url("https://example.com/specs/api.yaml");
    let cases = [
        (&https_spec, json!({}), Ok("https://example.com/")),
        (
            &https_spec,
            json!({"servers": [{"url": "/v{version}", "variables": {"version": {"default": "2"}}}]}),
            Ok("https://example.com/v2"),
        ),
        (
            &https_spec,
            json!({"servers": [{"url": "api/v1"}]}),
            Ok("https://example.com/specs/api/v1"),
        ),
        (
            &https_spec,
            json!({"servers": [{"url": "https://{region}.example.com/{base}", "variables": {"region": {"default": "eu"}}}]}),
            Err(ServerUrlError::MissingVariable {
                url: "https://{region}.example.com/{base}".to_owned(),
                name: "base".to_owned(),
            }),
        ),
        (
            &url("file:///home/user/api.yaml"),
            json!({"servers": [{"url": "/v1"}]}),
            Err(ServerUrlError::NotHttp {
                url: "file:///v1".to_owned(),
            }),
        ),
    ];
    for (spec_url, root, expected) in cases {
        let resolved = resolve_spec(&root, spec_url);
        assert_eq!(
            (&root, resolved),
            (&root, vec![expected.map(url)]),
            "spec at {spec_url}"
        );
    }
}
