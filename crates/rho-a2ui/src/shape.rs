//! How the protocol reads an authored component or function definition: the
//! properties a component has once its mixins are merged, and the argument
//! schema of a function.

use std::collections::{BTreeSet, HashSet};

use serde_json::{Map, Value};

use crate::schema;

/// Keys of the component envelope, which the protocol declares itself.
const ENVELOPE_KEYS: [&str; 4] = ["id", "component", "catalogId", "@catalogId"];

/// The common types a component mixes in for their properties.
const STANDARD_MIXINS: [&str; 2] = ["ComponentCommon", "Checkable"];

/// A component definition with its `allOf` members merged.
#[derive(Debug, Default)]
pub(crate) struct ComponentShape {
    /// Every property, a more specific piece overriding a more generic one,
    /// without the envelope keys. The schemas are as authored.
    pub(crate) properties: Map<String, Value>,
    /// The union of the pieces' `required` lists, without the envelope keys.
    pub(crate) required: BTreeSet<String>,
    /// The property names the component declares itself rather than through
    /// a common types mixin, envelope keys included.
    pub(crate) own: BTreeSet<String>,
    /// The most specific description.
    pub(crate) description: Option<String>,
}

/// A piece of a component definition that declares properties.
enum Piece<'a> {
    /// A common types mixin, such as `Checkable`. Its description describes
    /// the mixin, not the component.
    Mixin(&'a Map<String, Value>),
    /// The definition itself, an inline `allOf` member, or a local definition.
    Own(&'a Map<String, Value>),
}

/// Reads `definition` as the protocol does: the properties of the common
/// types mixins its `allOf` references, of the local definitions it
/// references, of its inline `allOf` members and, last, its own.
pub(crate) fn component_shape(
    definition: &Map<String, Value>,
    defs: &Map<String, Value>,
) -> ComponentShape {
    let mut shape = ComponentShape::default();
    for piece in pieces(definition, defs, &mut HashSet::new()) {
        let (Piece::Mixin(map) | Piece::Own(map)) = piece;
        if let Some(Value::Object(properties)) = map.get("properties") {
            for (name, property) in properties {
                shape.properties.insert(name.clone(), property.clone());
                if matches!(piece, Piece::Own(_)) {
                    shape.own.insert(name.clone());
                }
            }
        }
        let names = map.get("required").and_then(Value::as_array);
        shape.required.extend(
            names
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        if let (Piece::Own(_), Some(Value::String(description))) = (&piece, map.get("description"))
        {
            shape.description = Some(description.clone());
        }
    }
    // A bare `catalogId` the component declares itself is an ordinary
    // property: the v1.0 envelope field is `@catalogId`.
    let own_catalog_id = shape.own.contains("catalogId");
    let is_envelope =
        |name: &str| ENVELOPE_KEYS.contains(&name) && !(name == "catalogId" && own_catalog_id);
    shape.properties.retain(|name, _| !is_envelope(name));
    shape.required.retain(|name| !is_envelope(name));
    shape
}

fn pieces<'a>(
    definition: &'a Map<String, Value>,
    defs: &'a Map<String, Value>,
    visited: &mut HashSet<&'a str>,
) -> Vec<Piece<'a>> {
    let mut mixins = Vec::new();
    let mut local = Vec::new();
    let mut inline = Vec::new();
    let members = definition.get("allOf").and_then(Value::as_array);
    for member in members.into_iter().flatten() {
        let Value::Object(member) = member else {
            continue;
        };
        let Some(reference) = member.get("$ref").and_then(Value::as_str) else {
            inline.extend(pieces(member, defs, visited));
            continue;
        };
        let mixin = STANDARD_MIXINS.into_iter().find(|name| {
            reference
                .strip_suffix(name)
                .is_some_and(|rest| rest.ends_with('/'))
        });
        if mixin.is_none() {
            if let Some(Value::Object(target)) = resolve_local(defs, reference) {
                if visited.insert(reference) {
                    local.extend(pieces(target, defs, visited));
                }
                continue;
            }
        }
        let name = mixin.or_else(|| external_definition(reference));
        if let Some(Value::Object(common)) = name.and_then(schema::common_type) {
            if common.get("properties").is_some_and(Value::is_object) {
                mixins.push(Piece::Mixin(common));
            }
        }
    }
    let mut all = mixins;
    all.append(&mut local);
    all.append(&mut inline);
    if definition.contains_key("properties") || definition.contains_key("description") {
        all.push(Piece::Own(definition));
    }
    all
}

/// The names of the properties `definition` declares as its own, wherever it
/// declares them: in its `properties`, in `allOf`, `oneOf` and `anyOf`
/// members at any depth, and in the local definitions those reference. The
/// standard `ComponentCommon` and `Checkable` mixins are not its own, nor are
/// a `catalogId` and a `metadata` of their standard shape declared inline
/// beside `id`, which is `ComponentCommon` written out.
///
/// During the v1.0 transition a bare `catalogId`, `accessibility`,
/// `metadata` or `checks` key is read as the `@` envelope key unless the
/// component declares a property of that name as its own.
pub(crate) fn own_property_names(
    definition: &Map<String, Value>,
    defs: &Map<String, Value>,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_own(definition, defs, &mut HashSet::new(), &mut names);
    names
}

fn collect_own<'a>(
    definition: &'a Map<String, Value>,
    defs: &'a Map<String, Value>,
    visited: &mut HashSet<&'a str>,
    names: &mut BTreeSet<String>,
) {
    if let Some(Value::Object(properties)) = definition.get("properties") {
        let inline_common = properties.contains_key("id");
        for (name, schema) in properties {
            if !(inline_common && is_standard_envelope(name, schema)) {
                names.insert(name.clone());
            }
        }
    }
    let mut members: Vec<&Value> = Vec::new();
    for keyword in ["allOf", "oneOf", "anyOf"] {
        members.extend(
            definition
                .get(keyword)
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        );
    }
    for member in members {
        let Value::Object(member) = member else {
            continue;
        };
        match member.get("$ref").and_then(Value::as_str) {
            None => collect_own(member, defs, visited, names),
            Some(reference) => {
                if let Some(Value::Object(target)) = resolve_local(defs, reference) {
                    if visited.insert(reference) {
                        collect_own(target, defs, visited, names);
                    }
                }
            }
        }
    }
}

/// Whether `schema` declares `name` as `ComponentCommon` does: a string
/// `catalogId`, or a `metadata` object whose only property is `extensions`.
fn is_standard_envelope(name: &str, schema: &Value) -> bool {
    let type_is = |kind: &str| schema.get("type").and_then(Value::as_str) == Some(kind);
    match name {
        "catalogId" => type_is("string"),
        "metadata" => {
            type_is("object")
                && schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_some_and(|properties| {
                        properties.len() == 1 && properties.contains_key("extensions")
                    })
        }
        _ => false,
    }
}

/// The value a local reference such as `#/$defs/Base` points at in a
/// catalog's `$defs`.
pub(crate) fn resolve_local<'a>(
    defs: &'a Map<String, Value>,
    reference: &str,
) -> Option<&'a Value> {
    let pointer = reference.strip_prefix("#/$defs/")?;
    let (name, rest) = pointer
        .split_once('/')
        .map_or((pointer, ""), |(name, rest)| (name, rest));
    let target = defs.get(&unescape(name))?;
    if rest.is_empty() {
        Some(target)
    } else {
        target.pointer(&format!("/{rest}"))
    }
}

/// The definition a reference into another document's `$defs` names, when it
/// names a whole definition: `DynamicString` for
/// `common_types.json#/$defs/DynamicString`.
fn external_definition(reference: &str) -> Option<&str> {
    let (document, fragment) = reference.split_once('#')?;
    let name = fragment
        .strip_prefix("/$defs/")
        .or_else(|| fragment.strip_prefix("/definitions/"))?;
    (!document.is_empty() && !name.is_empty() && !name.contains('/')).then_some(name)
}

/// Decodes one JSON Pointer token (RFC 6901).
pub(crate) fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// The argument schema a function definition declares: its `args` property,
/// else its `args` or `parameters` key, where a bare map of argument schemas
/// stands for an object with those properties. A definition that is not a
/// call schema is its own argument schema. Anything else declares none, `{}`.
pub(crate) fn function_args(definition: &Map<String, Value>) -> Value {
    let properties = definition.get("properties").and_then(Value::as_object);
    let explicit = properties
        .and_then(|properties| properties.get("args"))
        .or_else(|| definition.get("args"))
        .or_else(|| definition.get("parameters"));
    if let Some(explicit) = explicit {
        return match explicit {
            Value::Object(map) if is_parameter_map(map) => {
                let mut args = Map::new();
                args.insert("type".to_owned(), Value::from("object"));
                args.insert("properties".to_owned(), explicit.clone());
                for key in ["required", "additionalProperties"] {
                    if let Some(value) = definition.get(key) {
                        args.insert(key.to_owned(), value.clone());
                    }
                }
                Value::Object(args)
            }
            Value::Object(_) => explicit.clone(),
            _ => Value::Object(Map::new()),
        };
    }
    let Some(properties) = properties else {
        return Value::Object(Map::new());
    };
    if ["call", "@call", "function"]
        .iter()
        .any(|key| properties.contains_key(*key))
    {
        return Value::Object(Map::new());
    }
    let describes_function = [
        "description",
        "title",
        "name",
        "returnType",
        "allowedCallers",
        "requiresUserActivation",
    ];
    Value::Object(
        definition
            .iter()
            .filter(|(key, _)| !describes_function.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// Whether `schema` is a bare map of argument name to schema rather than a
/// schema of its own.
fn is_parameter_map(schema: &Map<String, Value>) -> bool {
    const SCHEMA_KEYS: [&str; 6] = ["properties", "$ref", "allOf", "oneOf", "anyOf", "not"];
    !schema.is_empty()
        && !schema.get("type").is_some_and(Value::is_string)
        && !SCHEMA_KEYS.iter().any(|key| schema.contains_key(*key))
        && schema
            .values()
            .all(|value| value.is_object() || value.is_boolean())
}

/// The argument names an argument schema declares.
pub(crate) fn argument_names(args: &Value) -> BTreeSet<&str> {
    let members = args.get("allOf").and_then(Value::as_array);
    std::iter::once(args)
        .chain(members.into_iter().flatten())
        .filter_map(|schema| schema.get("properties").and_then(Value::as_object))
        .flat_map(Map::keys)
        .map(String::as_str)
        .collect()
}
