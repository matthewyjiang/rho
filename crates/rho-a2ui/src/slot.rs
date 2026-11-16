//! What a component schema says each property holds: references to child
//! components, an action, or a value, which may be dynamic. A renderer binds
//! each kind differently, so the schema decides, never the property's name.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::schema;
use crate::shape;

/// What a property holds, by its schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Slot {
    /// A value: a literal, a binding or a call.
    Value,
    /// One child component's id (`ComponentId`, `Child`).
    Child,
    /// A `ChildList`: ids, or a template over a data model list.
    ChildList,
    /// An `Action`.
    Action,
    /// A list whose items hold one of the kinds above.
    List(Box<Slot>),
    /// An object some of whose members hold one of the kinds above.
    Object(BTreeMap<String, Slot>),
}

/// The slot of each property in `properties` that holds anything but a
/// value. A property left out holds a value.
pub(crate) fn slots(
    properties: &Map<String, Value>,
    defs: &Map<String, Value>,
) -> BTreeMap<String, Slot> {
    properties
        .iter()
        .map(|(name, schema)| (name.clone(), slot(schema, defs, &mut Vec::new())))
        .filter(|(_, slot)| *slot != Slot::Value)
        .collect()
}

fn slot<'a>(schema: &'a Value, defs: &'a Map<String, Value>, visited: &mut Vec<&'a str>) -> Slot {
    let Value::Object(map) = schema else {
        return Slot::Value;
    };
    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
        if let Some(target) = shape::resolve_local(defs, reference) {
            if visited.contains(&reference) {
                return Slot::Value;
            }
            visited.push(reference);
            let found = slot(target, defs, visited);
            visited.pop();
            return found;
        }
        return match common_type(reference) {
            Some("ComponentId" | "Child") => Slot::Child,
            Some("ChildList") => Slot::ChildList,
            Some("Action") => Slot::Action,
            _ => Slot::Value,
        };
    }
    for keyword in ["allOf", "oneOf", "anyOf"] {
        let members = map.get(keyword).and_then(Value::as_array);
        for member in members.into_iter().flatten() {
            let found = slot(member, defs, visited);
            if found != Slot::Value {
                return found;
            }
        }
    }
    if let Some(items) = map.get("items") {
        let item = slot(items, defs, visited);
        if item != Slot::Value {
            return Slot::List(Box::new(item));
        }
    }
    if let Some(Value::Object(properties)) = map.get("properties") {
        let members: BTreeMap<String, Slot> = properties
            .iter()
            .map(|(name, member)| (name.clone(), slot(member, defs, visited)))
            .filter(|(_, slot)| *slot != Slot::Value)
            .collect();
        if !members.is_empty() {
            return Slot::Object(members);
        }
    }
    Slot::Value
}

/// The common type a reference names: `common_types.json#/$defs/X`, or a
/// local `#/$defs/X` the catalog does not define itself.
fn common_type(reference: &str) -> Option<&str> {
    let (document, fragment) = reference.split_once('#')?;
    if !document.is_empty() && !document.ends_with("common_types.json") {
        return None;
    }
    let name = fragment.strip_prefix("/$defs/")?;
    schema::common_type(name).map(|_| name)
}
