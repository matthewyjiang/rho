use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{response::MAX_RESPONSE_BYTES, test_server::load_test_source};
use super::*;

// Covers: only GET and HEAD operations the filter admits become tools, and the
// report counts them with their tags, most used first.
// Owner: OpenAPI bundle assembly
#[tokio::test]
async fn only_admitted_read_operations_become_tools() {
    let operation = |id: &str, tags: &[&str]| json!({"operationId": id, "tags": tags, "responses": {"200": {"description": "ok"}}});
    let spec = json!({
        "openapi": "3.1.0",
        "info": {"title": "test", "version": "1"},
        "servers": [{"url": "https://api.example.com"}],
        "paths": {
            "/items": {
                "get": operation("listItems", &["items"]),
                "head": operation("checkItems", &["items", "meta"]),
                "post": operation("createItem", &["items"]),
            },
            "/items/{id}": {
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "get": operation("getItem", &["items"]),
                "put": operation("replaceItem", &["items"]),
                "delete": operation("deleteItem", &["items"]),
                "patch": operation("updateItem", &["items"]),
            },
            "/admin": {"get": operation("adminStats", &["admin"])},
        },
    });
    let outcome =
        load_test_source(&spec, r#"deny = ["GET /admin"]"#, &[], MAX_RESPONSE_BYTES).await;

    let names: Vec<String> = outcome
        .bundle
        .as_ref()
        .map(|bundle| bundle.tools().iter().map(|tool| tool.spec().name).collect())
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![
            "openapi__test__listItems",
            "openapi__test__checkItems",
            "openapi__test__getItem",
        ]
    );
    assert_eq!(
        outcome.report,
        OpenApiReport {
            sources: vec![LoadedSource {
                name: "test".into(),
                read_operations: 3,
                tags: vec!["items".into(), "meta".into()],
            }],
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    );
}
