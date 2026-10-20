use pretty_assertions::assert_eq;
use serde_json::json;
use url::Url;

use super::super::{OpenApiSpec, SpecInput, DEFAULT_MAX_SPEC_BYTES};
use super::*;

// Covers: globs match the operationId or `METHOD /path`, `*` crosses `/`,
// deny wins over allow, and an empty allow admits everything not denied.
// Owner: OpenAPI operation filter
#[test]
fn deny_wins_and_empty_allow_admits_everything() {
    let ok = json!({"200": {"description": "ok"}});
    let spec = json!({
        "openapi": "3.1.0",
        "info": {"title": "test", "version": "1"},
        "paths": {
            "/repos/rust/issues": {"get": {"operationId": "issues/list-for-repo", "responses": ok}},
            "/repos/rust/pulls": {"get": {"operationId": "pulls/list", "responses": ok}},
            "/user": {"get": {"responses": ok}},
        },
    })
    .to_string();
    let spec = OpenApiSpec::load(SpecInput {
        name: "test",
        url: &Url::parse("https://api.example.com/spec.json").unwrap(),
        bytes: spec.as_bytes(),
        max_spec_bytes: DEFAULT_MAX_SPEC_BYTES,
    })
    .unwrap();
    let strings = |patterns: &[&str]| {
        patterns
            .iter()
            .map(|&pattern| pattern.to_owned())
            .collect::<Vec<_>>()
    };
    let cases: [(&[&str], &[&str], &[&str]); 6] = [
        (&[], &[], &["issues", "pulls", "user"]),
        (&["GET /repos/*"], &[], &["issues", "pulls"]),
        (&["issues/*"], &[], &["issues"]),
        (&[], &["pulls/*", "GET /user"], &["issues"]),
        (&["GET /repos/*"], &["*/list"], &["issues"]),
        (&["get /user", "issues/list-for-rep?"], &[], &["issues"]),
    ];
    for (allow, deny, expected) in cases {
        let filter = OperationFilter::new(&strings(allow), &strings(deny));
        let admitted: Vec<&str> = spec
            .operations
            .iter()
            .filter(|operation| filter.admits(operation))
            .map(|operation| operation.path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(admitted, expected, "allow {allow:?} deny {deny:?}");
    }
}
