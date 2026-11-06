//! The protocol as one JSON Schema document (#1657): every method's params
//! and result, every notification's params, and every error code, at
//! [`PROTOCOL_VERSION`].
//!
//! `schema/protocol.json` in this crate is [`document`], committed so that
//! clients in other languages can generate types from it; a test fails when
//! it is stale. Regenerate it with:
//!
//! ```text
//! cargo run -p rho-protocol --example protocol_schema > crates/rho-protocol/schema/protocol.json
//! ```
//!
//! CI compares it with the last release's (`scripts/check_protocol_compat.py`):
//! a change a protocol minor does not allow fails unless the major rises, and
//! any other change needs the minor to rise.

use schemars::{generate::SchemaSettings, SchemaGenerator};
use serde_json::{json, Map, Value};

use crate::{
    error::ALL,
    methods::{self, Method, Notification, Visitor},
    PROTOCOL_VERSION,
};

/// The marker on the `Unknown` variant of an enum that reads every value it
/// does not know as `Unknown` (`#[serde(other)]`), so a protocol minor may
/// add values to it.
const OTHER: &str = "x-rho-other";

/// The protocol's JSON Schema (draft 2020-12). Wire types are under
/// `$defs`; `methods` and `notifications` name them by method name, and
/// `errors` by wire code.
pub fn document() -> Value {
    generate(SchemaSettings::draft2020_12())
}

/// The document as `settings` describe the types. Their default contract is
/// what the daemon accepts; a test holds what it sends to the same shape.
fn generate(settings: SchemaSettings) -> Value {
    let mut publisher = Publisher {
        generator: settings.into_generator(),
        methods: Map::new(),
        notifications: Map::new(),
    };
    methods::visit(&mut publisher);
    let Publisher {
        mut generator,
        methods,
        notifications,
    } = publisher;
    let errors: Map<String, Value> = ALL
        .iter()
        .map(|&code| {
            let mut error = json!({ "name": code.name() });
            if let Some(data) = code.data_schema(&mut generator) {
                error["data"] = data.to_value();
            }
            (code.code().to_string(), error)
        })
        .collect();
    let mut definitions = generator.take_definitions(/*apply_transforms*/ true);
    definitions.values_mut().for_each(widen_other);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "rho daemon protocol",
        "protocol": PROTOCOL_VERSION,
        "methods": methods,
        "notifications": notifications,
        "errors": errors,
        "$defs": definitions,
    })
}

/// Makes an enum's [`OTHER`] variant match what serde reads as it: any
/// string, or for an internally tagged enum any tag. Its variants then
/// overlap, so they become `anyOf`.
fn widen_other(schema: &mut Value) {
    let Some(variants) = schema.get_mut("oneOf").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(other) = variants
        .iter_mut()
        .find(|variant| variant.get(OTHER).is_some())
    else {
        return;
    };
    let other = other
        .as_object_mut()
        .expect("a variant schema is an object");
    if other.remove("const").is_none() {
        let tag = other
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .and_then(|fields| {
                fields
                    .values_mut()
                    .find(|field| field.get("const").is_some())
            })
            .expect("an `Unknown` variant is a unit variant: a value or a tag");
        *tag = json!({ "type": "string" });
    }
    other.insert(
        "description".to_owned(),
        "Any other value: one a later protocol minor adds.".into(),
    );
    let schema = schema.as_object_mut().expect("an enum schema is an object");
    let variants = schema.remove("oneOf").expect("checked above");
    schema.insert("anyOf".to_owned(), variants);
}

/// Collects every method's and notification's schema; their types go to
/// the generator's definitions.
struct Publisher {
    generator: SchemaGenerator,
    methods: Map<String, Value>,
    notifications: Map<String, Value>,
}

impl Visitor for Publisher {
    fn method<M: Method>(&mut self) {
        let schema = json!({
            "params": self.generator.subschema_for::<M::Params>().to_value(),
            "result": self.generator.subschema_for::<M::Result>().to_value(),
        });
        self.methods.insert(M::NAME.to_owned(), schema);
    }

    fn notification<N: Notification>(&mut self) {
        let schema = json!({ "params": self.generator.subschema_for::<N::Params>().to_value() });
        self.notifications.insert(N::NAME.to_owned(), schema);
    }
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
