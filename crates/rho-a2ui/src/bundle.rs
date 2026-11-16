//! A catalog's validation schema: one self-contained JSON Schema document
//! with every component flattened and every common type it uses inlined.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::catalog::{union, Catalog, ComponentDef, EntryKind, FunctionDef};
use crate::schema;
use crate::shape;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// Keywords a reference to a common type keeps in a component property: the
/// annotations. The common type alone defines the value.
const ANNOTATIONS: [&str; 7] = [
    "description",
    "title",
    "default",
    "deprecated",
    "readOnly",
    "writeOnly",
    "examples",
];

/// The common types whose values may be a data binding or a function call.
const DYNAMIC_TYPES: [&str; 4] = [
    "DynamicString",
    "DynamicNumber",
    "DynamicBoolean",
    "DynamicValue",
];

/// `$defs` the validation schema computes rather than copies.
const COMPUTED_DEFS: [&str; 2] = ["anyComponent", "anyFunction"];

impl Catalog {
    /// The self-contained JSON Schema that validates this catalog's
    /// components and function calls.
    ///
    /// Each component is flattened into one object schema: the properties
    /// of the common types it mixes in and of its `allOf` members merged with
    /// its own, plus the `id` envelope. Every reference to a common type
    /// becomes a local `#/$defs/<name>` reference, the common types and leaf
    /// definitions reached are copied into `$defs`, and `anyComponent` and
    /// `anyFunction` list this catalog's entries. Nothing is fetched.
    #[must_use]
    pub fn validation_schema(&self) -> Value {
        Value::Object(self.validation_document())
    }

    pub(crate) fn validation_document(&self) -> Map<String, Value> {
        let no_defs = Map::new();
        let authored = self.defs().unwrap_or(&no_defs);

        let mut bundle = Map::new();
        bundle.insert("$schema".to_owned(), Value::from(DRAFT_2020_12));
        bundle.insert(
            "protocolVersion".to_owned(),
            Value::from(self.protocol_version()),
        );
        bundle.insert("catalogId".to_owned(), Value::from(self.id()));
        if let Some(instructions) = self.instructions() {
            bundle.insert("instructions".to_owned(), Value::from(instructions));
        }

        let mut defs = Map::new();
        let components: Map<String, Value> = self
            .components()
            .iter()
            .map(|(name, def)| (name.clone(), component(name, def, authored)))
            .collect();
        if !components.is_empty() {
            let names = self.ordered_names(EntryKind::Component);
            defs.insert(
                "anyComponent".to_owned(),
                union(EntryKind::Component, names),
            );
        }
        bundle.insert("components".to_owned(), Value::Object(components));
        if !self.functions().is_empty() {
            let functions = self
                .functions()
                .iter()
                .map(|(name, def)| (name.clone(), function(name, def, authored)))
                .collect();
            bundle.insert("functions".to_owned(), Value::Object(functions));
            let names = self.ordered_names(EntryKind::Function);
            defs.insert("anyFunction".to_owned(), union(EntryKind::Function, names));
        }

        for value in bundle.values_mut().chain(defs.values_mut()) {
            localize_refs(value);
        }
        add_referenced_defs(&bundle, &mut defs, authored);
        if !defs.is_empty() {
            bundle.insert("$defs".to_owned(), Value::Object(defs));
        }
        bundle
    }
}

/// Copies into `defs` every common type and leaf definition that `bundle`
/// and `defs` reference, until nothing new is reached.
fn add_referenced_defs(
    bundle: &Map<String, Value>,
    defs: &mut Map<String, Value>,
    authored: &Map<String, Value>,
) {
    loop {
        let before = defs.len();
        let mut referenced = BTreeSet::new();
        for def in bundle.values().chain(defs.values()) {
            collect_def_refs(def, &mut referenced);
        }
        // A dynamic value may be a binding or a call even where nothing names
        // those types directly.
        if DYNAMIC_TYPES.iter().any(|name| referenced.contains(*name)) {
            referenced.insert("DataBinding".to_owned());
            referenced.insert("FunctionCall".to_owned());
        }
        for name in &referenced {
            if defs.contains_key(name) {
                continue;
            }
            if let Some(common) = schema::common_type(name) {
                let mut copy = common.clone();
                localize_refs(&mut copy);
                defs.insert(name.clone(), copy);
            } else if let Some(leaf) = authored
                .get(name)
                .filter(|_| !COMPUTED_DEFS.contains(&name.as_str()))
            {
                let mut copy = leaf.clone();
                restore_refs(&mut copy, Restore::Whole);
                localize_refs(&mut copy);
                defs.insert(name.clone(), copy);
            }
        }
        // FunctionCall names the catalog's function union; a catalog without
        // functions gets the empty one.
        if referenced.contains("anyFunction") && !defs.contains_key("anyFunction") {
            defs.insert("anyFunction".to_owned(), json!({ "not": {} }));
        }
        if defs.len() == before {
            return;
        }
    }
}

/// One component flattened into an object schema with the `id` envelope.
fn component(name: &str, def: &ComponentDef, authored: &Map<String, Value>) -> Value {
    let no_definition = Map::new();
    let definition = def.schema().as_object().unwrap_or(&no_definition);
    let shape = shape::component_shape(definition, authored);

    let mut id = Map::new();
    localize_common_ref(&mut id, "ComponentId", "");
    let mut properties = Map::new();
    properties.insert("id".to_owned(), Value::Object(id));
    for (property, schema) in shape.properties {
        let mut schema = schema;
        restore_refs(&mut schema, Restore::AnnotationsOnly);
        properties.insert(property, schema);
    }
    properties.insert("component".to_owned(), json!({ "const": name }));
    let required: Vec<Value> = std::iter::once("id")
        .chain(shape.required.iter().map(String::as_str))
        .chain(std::iter::once("component"))
        .map(Value::from)
        .collect();

    let mut flat = Map::new();
    flat.insert("type".to_owned(), Value::from("object"));
    if let Some(description) = shape.description {
        flat.insert("description".to_owned(), Value::from(description));
    }
    flat.insert("properties".to_owned(), Value::Object(properties));
    flat.insert("required".to_owned(), Value::Array(required));
    if let Some(closure) = definition
        .get("unevaluatedProperties")
        .or_else(|| definition.get("additionalProperties"))
    {
        flat.insert("unevaluatedProperties".to_owned(), closure.clone());
    }
    for (field, names) in [
        ("allowedParents", def.allowed_parents()),
        ("allowedChildren", def.allowed_children()),
    ] {
        if let Some(names) = names.filter(|names| !names.is_empty()) {
            flat.insert(field.to_owned(), json!(names));
        }
    }
    Value::Object(flat)
}

/// The schema of a call to one function: `@call` naming it and its `args`
/// closed to the arguments it declares. The call stays open itself, since
/// `FunctionCall` adds the envelope keys around it.
fn function(name: &str, def: &FunctionDef, authored: &Map<String, Value>) -> Value {
    let mut args = def.args().clone();
    restore_refs(&mut args, Restore::Whole);
    if let Value::Object(map) = &mut args {
        if map.get("type").and_then(Value::as_str) == Some("object") {
            let additional = map.remove("additionalProperties");
            if !map.contains_key("unevaluatedProperties") {
                let closure = additional.unwrap_or(Value::Bool(false));
                map.insert("unevaluatedProperties".to_owned(), closure);
            }
        }
    }
    let required = if requires_args(&args, authored, &mut BTreeSet::new()) {
        json!(["@call", "args"])
    } else {
        json!(["@call"])
    };

    let definition = def.schema();
    let mut call = Map::new();
    call.insert("type".to_owned(), Value::from("object"));
    if let Some(description) = definition
        .get("description")
        .and_then(Value::as_str)
        .filter(|description| !description.is_empty())
    {
        call.insert("description".to_owned(), Value::from(description));
    }
    call.insert(
        "returnType".to_owned(),
        Value::from(def.return_type().as_str()),
    );
    for field in ["allowedCallers", "requiresUserActivation"] {
        if let Some(value) = definition.get(field) {
            call.insert(field.to_owned(), value.clone());
        }
    }
    call.insert(
        "properties".to_owned(),
        json!({ "@call": { "const": name }, "args": args }),
    );
    call.insert("required".to_owned(), required);
    Value::Object(call)
}

/// Whether a call must pass `args` for `schema` to accept them: the schema
/// requires a property, or every branch of a union does.
fn requires_args(
    schema: &Value,
    defs: &Map<String, Value>,
    visited: &mut BTreeSet<String>,
) -> bool {
    let Value::Object(map) = schema else {
        return false;
    };
    if map.contains_key("default") {
        return false;
    }
    if map
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|names| !names.is_empty())
        || map
            .get("minProperties")
            .and_then(Value::as_u64)
            .is_some_and(|min| min > 0)
    {
        return true;
    }
    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
        if visited.insert(reference.to_owned()) {
            if let Some(target) = shape::resolve_local(defs, reference) {
                if requires_args(target, defs, visited) {
                    return true;
                }
            }
        }
    }
    let branches = |keyword: &str| {
        map.get(keyword)
            .and_then(Value::as_array)
            .filter(|b| !b.is_empty())
    };
    branches("allOf").is_some_and(|all| {
        all.iter()
            .any(|sub| requires_args(sub, defs, &mut visited.clone()))
    }) || ["oneOf", "anyOf"].iter().any(|keyword| {
        branches(keyword).is_some_and(|all| {
            all.iter()
                .all(|sub| requires_args(sub, defs, &mut visited.clone()))
        })
    })
}

/// How [`restore_refs`] treats a reference to a common type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Restore {
    /// Keep only the reference and its annotations, as in a component
    /// property, where the common type alone defines the value.
    AnnotationsOnly,
    /// Keep the node whole.
    Whole,
}

/// Points every reference to a common type below `node`, written either way
/// (`common_types.json#/$defs/X` or `#/$defs/X` for a common type `X`), at
/// the bundled `#/$defs/X`. A reference to a whole common type without a
/// description of its own takes the common type's.
fn restore_refs(node: &mut Value, restore: Restore) {
    match node {
        Value::Array(items) => {
            for item in items {
                restore_refs(item, restore);
            }
        }
        Value::Object(map) => {
            let target = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(common_type_ref)
                .map(|(name, pointer)| (name.to_owned(), pointer.to_owned()));
            if let Some((name, pointer)) = target {
                if restore == Restore::AnnotationsOnly {
                    map.retain(|key, _| key == "$ref" || ANNOTATIONS.contains(&key.as_str()));
                    localize_common_ref(map, &name, &pointer);
                    return;
                }
                localize_common_ref(map, &name, &pointer);
            }
            for value in map.values_mut() {
                restore_refs(value, restore);
            }
        }
        _ => {}
    }
}

/// The common type a reference names, and the pointer below it: from
/// `common_types.json#/$defs/X/oneOf/0`, `X` and `/oneOf/0`. A local
/// `#/$defs/X` names a common type only when one is called `X`.
fn common_type_ref(reference: &str) -> Option<(&str, &str)> {
    let (document, fragment) = reference.split_once('#')?;
    if !document.is_empty() && !document.ends_with("common_types.json") {
        return None;
    }
    let pointer = fragment.strip_prefix("/$defs/")?;
    let (name, rest) = pointer
        .find('/')
        .map_or((pointer, ""), |at| pointer.split_at(at));
    let is_common = !document.is_empty() || schema::common_type(name).is_some();
    (!name.is_empty() && is_common).then_some((name, rest))
}

/// Points `node` at the bundled common type `name`, or at `pointer` within
/// it. A reference to the whole type without a description takes its
/// description.
fn localize_common_ref(node: &mut Map<String, Value>, name: &str, pointer: &str) {
    node.insert(
        "$ref".to_owned(),
        Value::from(format!("#/$defs/{name}{pointer}")),
    );
    if !pointer.is_empty() || node.contains_key("description") {
        return;
    }
    if let Some(description) =
        schema::common_type(name).and_then(|common| common.get("description"))
    {
        if description.is_string() {
            node.insert("description".to_owned(), description.clone());
        }
    }
}

/// Rewrites every reference into another document's `$defs`, such as
/// `catalog.json#/$defs/anyFunction`, as the local `#/$defs/anyFunction`.
fn localize_refs(node: &mut Value) {
    match node {
        Value::Array(items) => {
            for item in items {
                localize_refs(item);
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                if let Some((document, fragment)) = reference.split_once('#') {
                    if !document.is_empty() && fragment.starts_with("/$defs/") {
                        *reference = format!("#{fragment}");
                    }
                }
            }
            for value in map.values_mut() {
                localize_refs(value);
            }
        }
        _ => {}
    }
}

/// Adds the name of every definition a local `#/$defs/...` reference below
/// `node` reaches.
fn collect_def_refs(node: &Value, referenced: &mut BTreeSet<String>) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect_def_refs(item, referenced);
            }
        }
        Value::Object(map) => {
            if let Some(pointer) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
            {
                let name = pointer.split('/').next().unwrap_or(pointer);
                referenced.insert(shape::unescape(name));
            }
            for value in map.values() {
                collect_def_refs(value, referenced);
            }
        }
        _ => {}
    }
}
