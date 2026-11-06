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
    methods::{
        Hello, JobList, Method, Notification, SessionAttach, SessionCancel, SessionDetach,
        SessionDetachedNotification, SessionEventNotification, SessionNew, SessionPrompt,
        SessionSteer, Status, Stop,
    },
    ErrorCode, VersionMismatch, PROTOCOL_VERSION,
};

/// The protocol's JSON Schema (draft 2020-12). Wire types are under
/// `$defs`; `methods`, `notifications`, and `errors` name them.
pub fn document() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let methods = Map::from_iter([
        method::<Hello>(&mut generator),
        method::<Status>(&mut generator),
        method::<Stop>(&mut generator),
        method::<SessionNew>(&mut generator),
        method::<SessionPrompt>(&mut generator),
        method::<SessionCancel>(&mut generator),
        method::<SessionSteer>(&mut generator),
        method::<SessionAttach>(&mut generator),
        method::<SessionDetach>(&mut generator),
        method::<JobList>(&mut generator),
    ]);
    let notifications = Map::from_iter([
        notification::<SessionEventNotification>(&mut generator),
        notification::<SessionDetachedNotification>(&mut generator),
    ]);
    let errors: Map<String, Value> = ALL
        .iter()
        .map(|&code| {
            let mut error = json!({ "code": code.code() });
            // The only code whose `data` has a shape.
            if code == ErrorCode::VersionMismatch {
                error["data"] = generator.subschema_for::<VersionMismatch>().to_value();
            }
            (format!("{code:?}"), error)
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

fn method<M: Method>(generator: &mut SchemaGenerator) -> (String, Value) {
    let schema = json!({
        "params": generator.subschema_for::<M::Params>().to_value(),
        "result": generator.subschema_for::<M::Result>().to_value(),
    });
    (M::NAME.to_owned(), schema)
}

fn notification<N: Notification>(generator: &mut SchemaGenerator) -> (String, Value) {
    let schema = json!({ "params": generator.subschema_for::<N::Params>().to_value() });
    (N::NAME.to_owned(), schema)
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
