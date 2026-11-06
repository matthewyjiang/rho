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

/// The protocol's JSON Schema (draft 2020-12). Wire types are under
/// `$defs`; `methods` and `notifications` name them by method name, and
/// `errors` by wire code.
pub fn document() -> Value {
    let mut publisher = Publisher {
        generator: SchemaSettings::draft2020_12().into_generator(),
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
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "rho daemon protocol",
        "protocol": PROTOCOL_VERSION,
        "methods": methods,
        "notifications": notifications,
        "errors": errors,
        "$defs": generator.take_definitions(/*apply_transforms*/ true),
    })
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
