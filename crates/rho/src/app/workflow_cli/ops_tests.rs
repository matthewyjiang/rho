use pretty_assertions::assert_eq;

use super::STARLARK_VERSION;

// Covers: a Starlark dependency update must also update the frozen planner identity.
// Owner: workflow plan freeze policy.
#[test]
fn frozen_planner_version_matches_starlark_dependency() {
    let manifest: toml::Value = toml::from_str(include_str!("../../../Cargo.toml")).unwrap();
    let dependency = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get("starlark"))
        .and_then(toml::Value::as_str);
    let expected = format!("={STARLARK_VERSION}");

    assert_eq!(dependency, Some(expected.as_str()));
}
