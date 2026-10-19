use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::{
    document::{JsonPointer, RefError},
    operation::{
        HttpMethod::{Delete, Get, Post, Put},
        JsonContent, RequestBody, ResponseBody, SkipReason, SuccessResponse,
    },
    params::{Param, ParamStyle, Params},
    schema::SchemaDirection,
    servers::{resolve_spec, ServerUrlError},
    *,
};

const SLOPPY: &[u8] = include_bytes!("openapi/fixtures/sloppy.yaml");
const ALIAS_BOMB: &[u8] = b"a: &a [1,1,1,1,1,1,1,1,1,1]
b: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a,*a]
c: [*b,*b,*b,*b,*b,*b,*b,*b,*b,*b]
";
const STRING_ALIAS_BOMB: &[u8] = b"a: &a xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
b: [*a,*a,*a,*a,*a,*a,*a,*a,*a,*a]
";
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
            ("openapi__sloppy__pets_replace", Put, "/pets/{id}"),
            // A recursive schema is legal.
            ("openapi__sloppy__tree_get", Get, "/tree"),
            // A path item behind a `$ref`, with a text-only response.
            ("openapi__sloppy__aliased", Get, "/aliased"),
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
        spec.operations[4].servers,
        Some(vec![Ok(url("https://trees.example.com"))])
    );
    assert_eq!(
        spec.operations[5].responses,
        vec![SuccessResponse {
            status: "200".to_owned(),
            body: ResponseBody::Text {
                media_type: "text/plain".to_owned()
            },
        }]
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
                SkipReason::NonJsonRequestBody {
                    declared: vec![name("application/octet-stream")]
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
            (
                Some(Get),
                "/report",
                SkipReason::BinaryResponse {
                    media_type: name("application/pdf")
                }
            ),
            (
                Some(Get),
                "/export",
                SkipReason::BinaryResponse {
                    media_type: name("text/csv")
                }
            ),
            (
                Some(Get),
                "/deep",
                RefError::TooDeep {
                    reference: name("#/components/parameters/limit"),
                    limit: 16
                }
                .into()
            ),
            (
                Some(Get),
                "/listless",
                SkipReason::InvalidParam("parameters is not a list")
            ),
            // `x-internal` under `paths` is an extension and is not reported.
            (None, "/bogus", SkipReason::NotAnObject),
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
                SkipReason::NonJsonRequestBody {
                    declared: vec!["application/octet-stream".to_owned()]
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
            at: pointer(&format!("#{comments}")),
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
                body: ResponseBody::Json(JsonContent {
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
            SchemaDirection::Response => match &operation.responses[0].body {
                ResponseBody::Json(content) => Some(content),
                ResponseBody::Empty | ResponseBody::Text { .. } => None,
            },
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
    // `id` is hidden through `allOf` and `owner` through an `allOf` wrapper, so
    // both leave `required`. Under `not`, Pet renders unchanged as its own
    // definition, since removing `id` there would change what it rejects.
    let new_pet = schema_of("openapi__sloppy__pets_replace", SchemaDirection::Request);
    assert_eq!(
        new_pet["$defs"]["NewPet"],
        json!({
            "allOf": [{"$ref": "#/$defs/Pet"}],
            "required": ["name"],
            "properties": {},
            "not": {"$ref": "#/$defs/Pet_2"},
        })
    );
    let required = |def: &str| new_pet["$defs"][def]["required"].clone();
    assert_eq!(
        (required("Pet"), required("Pet_2")),
        (json!(["name", "secret"]), json!(["id", "name", "secret"]))
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
        // A 3.0 chain collapses to its end; in 3.1 each link keeps its own
        // constraints.
        (
            "3.0.3",
            json!({"$ref": "#/components/schemas/c"}),
            json!({"$ref": "#/$defs/t", "$defs": target_defs}),
        ),
        (
            "3.1.0",
            json!({"$ref": "#/components/schemas/c"}),
            json!({"$ref": "#/$defs/c", "$defs": {
                "c": {"$ref": "#/$defs/t", "maxLength": 3},
                "t": {"type": "string"},
            }}),
        ),
    ];
    for (version, schema, expected) in cases {
        let spec = json!({
            "openapi": version,
            "paths": {},
            "components": {"schemas": {
                "s": schema,
                "t": {"type": "string"},
                "c": {"$ref": "#/components/schemas/t", "maxLength": 3},
            }},
        });
        let document = Document::parse(spec.to_string().as_bytes()).expect("parses");
        let rendered = document
            .render_schema(&pointer("#/components/schemas/s"), SchemaDirection::Request)
            .expect("renders");
        assert_eq!((version, &schema, rendered), (version, &schema, expected));
    }
}

// Covers: in 3.1, refs beside a `$ref` are checked too, so a broken one skips
// the operation instead of failing later at render time.
// Owner: OpenAPI operation index.
#[test]
fn openapi_3_1_checks_refs_beside_a_ref() {
    let spec = json!({
        "openapi": "3.1.0",
        "paths": {"/thing": {"get": {
            "operationId": "thing",
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "$ref": "#/components/schemas/base",
                "properties": {"x": {"$ref": "#/components/schemas/missing"}},
            }}}}},
        }}},
        "components": {"schemas": {"base": {"type": "object"}}},
    });
    let spec = load("api", spec.to_string().as_bytes());
    let skipped: Vec<_> = spec
        .skipped
        .iter()
        .map(|skipped| skipped.reason.clone())
        .collect();
    assert_eq!(
        (spec.operations.len(), skipped),
        (
            0,
            vec![RefError::Broken("#/components/schemas/missing".to_owned()).into()]
        )
    );
}

// Covers: only an oversized, unparsable, alias-bombed, or non-3.x document
// fails the whole source, and size errors name their limit.
// Owner: OpenAPI spec loader.
#[test]
fn load_fails_whole_source_only_for_document_problems() {
    type IsExpected = fn(&LoadError) -> bool;
    let cases: [(&[u8], usize, IsExpected); 8] = [
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
        // Each alias replays the list above it, so about 100 bytes expand to
        // over 1,200 nodes.
        (
            ALIAS_BOMB,
            1024,
            |error| matches!(error, LoadError::YamlAliasExpansion { limit } if *limit == ALIAS_BOMB.len()),
        ),
        // Few nodes, but each alias replays a 40-byte string.
        (
            STRING_ALIAS_BOMB,
            1024,
            |error| matches!(error, LoadError::YamlAliasExpansion { limit } if *limit == STRING_ALIAS_BOMB.len()),
        ),
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
            &https_spec,
            json!({"servers": [{"url": "https://example.com:{port}", "variables": {"port": {"default": 8443}}}]}),
            Ok("https://example.com:8443/"),
        ),
        (
            &https_spec,
            json!({"servers": [{"url": "https://example.com/api?version=2"}]}),
            Err(ServerUrlError::QueryOrFragment {
                url: "https://example.com/api?version=2".to_owned(),
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
