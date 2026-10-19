//! Schema traversal: `$ref` checks at index time and on-demand rendering.
//!
//! A rendered schema is self-contained JSON Schema 2020-12. Every local ref is
//! copied once into a root `$defs` and rewritten to point there, so recursive
//! schemas stay finite and shared components are not inlined repeatedly.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::document::{Document, JsonPointer, RefError, SpecVersion};
use crate::tools::tool_name::sanitize_segment;

/// Which side of a call a schema describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaDirection {
    /// Parameters and request bodies: `readOnly` properties are dropped.
    Request,
    /// Response bodies: `writeOnly` properties are dropped.
    Response,
}

impl SchemaDirection {
    fn hidden_keyword(self) -> &'static str {
        match self {
            Self::Request => "readOnly",
            Self::Response => "writeOnly",
        }
    }
}

/// Keywords holding one subschema, or a list of them (`items` in 2019 form).
const SUBSCHEMA_KEYWORDS: &[&str] = &[
    "items",
    "additionalItems",
    "additionalProperties",
    "unevaluatedItems",
    "unevaluatedProperties",
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
    "allOf",
    "anyOf",
    "oneOf",
    "prefixItems",
];

/// Keywords mapping names to subschemas.
const SUBSCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "dependentSchemas",
    "$defs",
    "definitions",
];

/// Calls `visit` on every direct subschema of `schema`.
fn for_each_subschema<E>(
    schema: &Map<String, Value>,
    mut visit: impl FnMut(&Value) -> Result<(), E>,
) -> Result<(), E> {
    for keyword in SUBSCHEMA_KEYWORDS {
        match schema.get(*keyword) {
            Some(Value::Array(items)) => items.iter().try_for_each(&mut visit)?,
            Some(subschema) => visit(subschema)?,
            None => {}
        }
    }
    for keyword in SUBSCHEMA_MAP_KEYWORDS {
        if let Some(Value::Object(entries)) = schema.get(*keyword) {
            entries.values().try_for_each(&mut visit)?;
        }
    }
    Ok(())
}

/// The mutable twin of [`for_each_subschema`].
fn for_each_subschema_mut<E>(
    schema: &mut Map<String, Value>,
    mut visit: impl FnMut(&mut Value) -> Result<(), E>,
) -> Result<(), E> {
    for (keyword, value) in schema.iter_mut() {
        let keyword = keyword.as_str();
        match value {
            Value::Array(items) if SUBSCHEMA_KEYWORDS.contains(&keyword) => {
                items.iter_mut().try_for_each(&mut visit)?;
            }
            Value::Object(entries) if SUBSCHEMA_MAP_KEYWORDS.contains(&keyword) => {
                entries.values_mut().try_for_each(&mut visit)?;
            }
            subschema if SUBSCHEMA_KEYWORDS.contains(&keyword) => visit(subschema)?,
            _ => {}
        }
    }
    Ok(())
}

/// Check that every `$ref` reachable from `roots` is local and resolves.
///
/// `verified` remembers schemas whose whole reachable graph already passed, so
/// operations sharing components walk them once. It only grows on success:
/// after a failure, part of the walk may sit on a cycle through the bad ref.
pub(super) fn check_refs(
    document: &Document,
    roots: Vec<JsonPointer>,
    verified: &mut HashSet<JsonPointer>,
) -> Result<(), RefError> {
    let mut pending = roots;
    let mut visited = HashSet::new();
    while let Some(at) = pending.pop() {
        if verified.contains(&at) || !visited.insert(at.clone()) {
            continue;
        }
        let (_, schema) = document.resolve_at(at)?;
        collect_refs(document, schema, &mut pending)?;
    }
    verified.extend(visited);
    Ok(())
}

/// Push the target of every ref inside `schema` without leaving it.
fn collect_refs(
    document: &Document,
    schema: &Value,
    pending: &mut Vec<JsonPointer>,
) -> Result<(), RefError> {
    let Value::Object(map) = schema else {
        return Ok(());
    };
    if let Some((target, _)) = document.resolve(schema)? {
        pending.push(target);
        if document.version() == SpecVersion::V3_0 {
            return Ok(());
        }
    }
    for_each_subschema(map, |subschema| collect_refs(document, subschema, pending))
}

impl Document {
    /// Render the schema at `at` as self-contained JSON Schema 2020-12, with
    /// properties hidden in `direction` removed.
    pub(crate) fn render_schema(
        &self,
        at: &JsonPointer,
        direction: SchemaDirection,
    ) -> Result<Value, RefError> {
        let root = self
            .get(at)
            .ok_or_else(|| RefError::Broken(at.to_string()))?;
        let mut renderer = Renderer {
            document: self,
            direction,
            def_keys: HashMap::new(),
            used_keys: HashSet::new(),
            pending: Vec::new(),
        };
        let mut rendered = root.clone();
        renderer.transform(&mut rendered)?;
        let mut defs = Map::new();
        while let Some((target, key)) = renderer.pending.pop() {
            let mut def = self
                .get(&target)
                .cloned()
                .ok_or_else(|| RefError::Broken(target.to_string()))?;
            renderer.transform(&mut def)?;
            defs.insert(key, def);
        }
        if !defs.is_empty() {
            defs.sort_keys();
            // Every ref was rewritten into `defs`, so a root `$defs` is unused.
            if let Value::Object(map) = &mut rendered {
                map.insert("$defs".to_owned(), Value::Object(defs));
            }
        }
        Ok(rendered)
    }
}

struct Renderer<'a> {
    document: &'a Document,
    direction: SchemaDirection,
    /// `$defs` key for each ref target already seen.
    def_keys: HashMap<JsonPointer, String>,
    used_keys: HashSet<String>,
    /// Targets whose definitions still need rendering.
    pending: Vec<(JsonPointer, String)>,
}

impl Renderer<'_> {
    fn transform(&mut self, schema: &mut Value) -> Result<(), RefError> {
        let def_key = self
            .document
            .resolve(schema)?
            .map(|(target, _)| self.def_key(target));
        let Value::Object(map) = schema else {
            return Ok(());
        };
        let is_3_0 = self.document.version() == SpecVersion::V3_0;
        if let Some(key) = def_key {
            if is_3_0 {
                // OpenAPI 3.0 ignores every sibling of `$ref`.
                map.clear();
            }
            // Keys never contain `~` or `/`, so they need no pointer escaping.
            map.insert("$ref".to_owned(), Value::String(format!("#/$defs/{key}")));
            if is_3_0 {
                return Ok(());
            }
        }
        if is_3_0 {
            normalize_3_0(map);
        }
        self.drop_hidden_properties(map)?;
        for_each_subschema_mut(map, |subschema| self.transform(subschema))
    }

    /// Remove properties marked hidden for this direction, directly or on the
    /// schema they reference, and drop them from `required`.
    fn drop_hidden_properties(&self, schema: &mut Map<String, Value>) -> Result<(), RefError> {
        let Some(Value::Object(properties)) = schema.get_mut("properties") else {
            return Ok(());
        };
        let keyword = self.direction.hidden_keyword();
        let mut hidden = Vec::new();
        let is_hidden = |value: &Value| value.get(keyword) == Some(&Value::Bool(true));
        for (name, property) in properties.iter() {
            let target = self.document.resolve(property)?.map(|(_, target)| target);
            if is_hidden(property) || target.is_some_and(is_hidden) {
                hidden.push(name.clone());
            }
        }
        if hidden.is_empty() {
            return Ok(());
        }
        properties.retain(|name, _| !hidden.contains(name));
        if let Some(Value::Array(required)) = schema.get_mut("required") {
            required.retain(|name| !hidden.iter().any(|hidden| name == hidden));
        }
        Ok(())
    }

    /// A stable, readable `$defs` key for `target`, queued on first sight.
    fn def_key(&mut self, target: JsonPointer) -> String {
        if let Some(key) = self.def_keys.get(&target) {
            return key.clone();
        }
        let last = target.segments().last().unwrap_or_default();
        let readable = !last.is_empty()
            && last
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character));
        let base = if readable {
            last
        } else {
            sanitize_segment(&last)
        };
        let mut key = base.clone();
        let mut suffix = 2;
        while !self.used_keys.insert(key.clone()) {
            key = format!("{base}_{suffix}");
            suffix += 1;
        }
        self.def_keys.insert(target.clone(), key.clone());
        self.pending.push((target, key.clone()));
        key
    }
}

/// Rewrite OpenAPI 3.0 keywords into their JSON Schema 2020-12 form.
fn normalize_3_0(schema: &mut Map<String, Value>) {
    for (flag, bound) in [
        ("exclusiveMinimum", "minimum"),
        ("exclusiveMaximum", "maximum"),
    ] {
        // 3.0 marks the plain bound exclusive; 2020-12 moves the number.
        if let Some(Value::Bool(exclusive)) = schema.get(flag) {
            let exclusive = *exclusive;
            schema.remove(flag);
            match schema.remove(bound) {
                Some(limit) if exclusive => schema.insert(flag.to_owned(), limit),
                Some(limit) => schema.insert(bound.to_owned(), limit),
                None => None,
            };
        }
    }
    if schema.remove("nullable") != Some(Value::Bool(true)) {
        return;
    }
    if let Some(kind) = schema.get_mut("type") {
        let null = Value::String("null".to_owned());
        match kind {
            Value::Array(kinds) if !kinds.contains(&null) => kinds.push(null),
            Value::String(_) => *kind = Value::Array(vec![kind.take(), null]),
            _ => {}
        }
        if let Some(Value::Array(values)) = schema.get_mut("enum") {
            if !values.contains(&Value::Null) {
                values.push(Value::Null);
            }
        }
        return;
    }
    // Without `type`, 3.0 gives `nullable` no meaning, but specs such as
    // GitHub's put it on a composition to mean "this, or null".
    if ["allOf", "anyOf", "oneOf"]
        .iter()
        .any(|keyword| schema.contains_key(*keyword))
    {
        let inner = std::mem::take(schema);
        let null = Value::Object(Map::from_iter([(
            "type".to_owned(),
            Value::String("null".to_owned()),
        )]));
        schema.insert(
            "anyOf".to_owned(),
            Value::Array(vec![Value::Object(inner), null]),
        );
    }
}
