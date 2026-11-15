use serde_json::json;

use super::*;

/// A call that names no catalog runs in the surface default. In a
/// component without `@catalogId` that is the validator's catalog, so an
/// unknown function is an error; in one that names a `@catalogId` the
/// default may be another catalog, so only the envelope is checked.
#[test]
fn a_call_without_catalog_id_runs_in_the_surface_default() {
    let validator = CatalogValidator::new(Catalog::basic()).expect("the basic catalog compiles");
    let text = json!({ "id": "t", "component": "Text",
                       "text": { "@call": "shout", "args": { "loud": true } } });
    let got = validator
        .validate_component(&text)
        .map_err(|e| e.violations().to_vec());
    let want = vec![Violation::UnknownFunction {
        path: "/text".to_owned(),
        name: "shout".to_owned(),
    }];
    assert_eq!(got, Err(want));

    let mut elsewhere = text;
    elsewhere["@catalogId"] = json!("https://example.com/other");
    assert_eq!(validator.validate_component(&elsewhere), Ok(()));
}
