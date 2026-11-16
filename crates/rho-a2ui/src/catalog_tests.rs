use serde_json::json;

use super::*;

/// Loading rules the conformance suite does not exercise: an unversioned
/// catalog is pre-1.0 and fails closed, `Surface` is reserved, and a
/// `$ref` must resolve without leaving the catalog and common types.
#[test]
fn loading_rejects_what_the_suite_leaves_out() {
    let cases = [
        (
            json!({ "catalogId": "c" }),
            CatalogError::UnsupportedVersion("0.9".to_owned()),
        ),
        (
            json!({ "catalogId": "c", "protocolVersion": "1.0", "components": { "Surface": {} } }),
            CatalogError::ReservedComponent,
        ),
        (
            json!({ "catalogId": "c", "protocolVersion": "1.0",
                    "components": { "A": { "$ref": "#/$defs/Missing" } } }),
            CatalogError::UnresolvedRef {
                at: "/components/A/$ref".to_owned(),
                reference: "#/$defs/Missing".to_owned(),
            },
        ),
        (
            json!({ "catalogId": "c", "protocolVersion": "1.0",
                    "functions": { "f": { "returnType": "string",
                        "args": { "$ref": "common_types.json#/$defs/Nope" } } } }),
            CatalogError::UnresolvedRef {
                at: "/functions/f/args/$ref".to_owned(),
                reference: "common_types.json#/$defs/Nope".to_owned(),
            },
        ),
    ];
    for (document, want) in cases {
        let got = Catalog::from_json(document.clone()).map(|catalog| catalog.to_json());
        assert_eq!(got, Err(want), "loading {document}");
    }
}
