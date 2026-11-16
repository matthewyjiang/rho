//! The published A2UI v1.0 schemas, compiled into the crate so that loading a
//! catalog or validating against one never reads a file or the network.

use std::sync::LazyLock;

use serde_json::Value;

/// The `$id` of the published v1.0 common types.
pub(crate) const COMMON_TYPES_URI: &str = "https://a2ui.org/specification/v1_0/common_types.json";

/// The published v1.0 basic catalog.
pub(crate) const BASIC_CATALOG: &str =
    include_str!("../vendor/a2ui/catalogs/basic/v1/catalog.json");

static COMMON_TYPES: LazyLock<Value> = LazyLock::new(|| {
    parse(
        "common_types.json",
        include_str!("../vendor/a2ui/specification/v1_0/json/common_types.json"),
    )
});

static AGENT_TO_RENDERER: LazyLock<Value> = LazyLock::new(|| {
    parse(
        "agent_to_renderer.json",
        include_str!("../vendor/a2ui/specification/v1_0/json/agent_to_renderer.json"),
    )
});

fn parse(name: &str, text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("the vendored {name} is not JSON: {e}"))
}

/// The published v1.0 `common_types.json`: the types catalogs build on.
#[must_use]
pub fn common_types() -> &'static Value {
    &COMMON_TYPES
}

/// The published v1.0 `agent_to_renderer.json`: the messages an agent sends.
#[must_use]
pub fn agent_to_renderer() -> &'static Value {
    &AGENT_TO_RENDERER
}

/// The common type `name`, from the `$defs` of [`common_types`].
pub(crate) fn common_type(name: &str) -> Option<&'static Value> {
    common_types().get("$defs")?.get(name)
}
