//! Schema traversal: `$ref` checks at index time and on-demand rendering.
//!
//! A rendered schema is self-contained JSON Schema 2020-12. Every local ref is
//! copied once into a root `$defs` and rewritten to point there, so recursive
//! schemas stay finite and shared components are not inlined repeatedly.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::{Map, Value};

use super::document::{Document, JsonPointer, RefError, SpecVersion};
use crate::tools::tool_name::sanitize_segment;

/// Hidden-property lookups follow `$ref` and `allOf` across the document, and
/// each definition repeats its own walk, so a long chain costs its length
/// squared: 12,000 links took 376 s to render. GitHub's deepest walk reaches
/// 3 schemas, so a walk past this many fails the render instead.
const MAX_ALLOF_REFS: usize = 64;

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
/// Both walkers below visit exactly these and [`SUBSCHEMA_MAP_KEYWORDS`], so
/// an operation whose refs pass [`check_refs`] always renders.
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
    for (keyword, value) in schema {
        let keyword = keyword.as_str();
        match value {
            Value::Array(items) if SUBSCHEMA_KEYWORDS.contains(&keyword) => {
                items.iter().try_for_each(&mut visit)?;
            }
            Value::Object(entries) if SUBSCHEMA_MAP_KEYWORDS.contains(&keyword) => {
                entries.values().try_for_each(&mut visit)?;
            }
            subschema if SUBSCHEMA_KEYWORDS.contains(&keyword) => visit(subschema)?,
            _ => {}
        }
    }
    Ok(())
}

/// The mutable twin of [`for_each_subschema`], which also passes the keyword
/// each subschema sits under.
fn for_each_subschema_mut<E>(
    schema: &mut Map<String, Value>,
    mut visit: impl FnMut(&str, &mut Value) -> Result<(), E>,
) -> Result<(), E> {
    for (keyword, value) in schema.iter_mut() {
        let keyword = keyword.as_str();
        match value {
            Value::Array(items) if SUBSCHEMA_KEYWORDS.contains(&keyword) => {
                for item in items {
                    visit(keyword, item)?;
                }
            }
            Value::Object(entries) if SUBSCHEMA_MAP_KEYWORDS.contains(&keyword) => {
                for entry in entries.values_mut() {
                    visit(keyword, entry)?;
                }
            }
            subschema if SUBSCHEMA_KEYWORDS.contains(&keyword) => visit(keyword, subschema)?,
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
        let schema = document
            .get(&at)
            .ok_or_else(|| RefError::Broken(at.to_string()))?;
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
    if let Some(target) = document.schema_ref_target(map)? {
        pending.push(target);
        if document.version().ignores_ref_siblings() {
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
        renderer.transform(&mut rendered, Hiding::On(&BTreeSet::new()))?;
        let mut defs = Map::new();
        while let Some((target, mode, key)) = renderer.pending.pop() {
            let mut def = self
                .get(&target)
                .cloned()
                .ok_or_else(|| RefError::Broken(target.to_string()))?;
            // A definition is shared by every use site, so it hides only
            // what it declares itself.
            let hiding = match mode {
                DefMode::Hiding => Hiding::On(&BTreeSet::new()),
                DefMode::Verbatim => Hiding::Off,
            };
            renderer.transform(&mut def, hiding)?;
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
    /// `$defs` key for each ref target already seen, in each mode.
    def_keys: HashMap<(JsonPointer, DefMode), String>,
    used_keys: HashSet<String>,
    /// Targets whose definitions still need rendering.
    pending: Vec<(JsonPointer, DefMode, String)>,
}

impl Renderer<'_> {
    fn transform(&mut self, schema: &mut Value, hiding: Hiding<'_>) -> Result<(), RefError> {
        let Value::Object(map) = schema else {
            return Ok(());
        };
        let target = self.document.schema_ref_target(map)?;
        let mode = match hiding {
            Hiding::On(_) => DefMode::Hiding,
            Hiding::Off => DefMode::Verbatim,
        };
        if let (Some(target), true) = (&target, self.document.version().ignores_ref_siblings()) {
            let reference = self.def_ref(target.clone(), mode);
            *map = Map::from_iter([("$ref".to_owned(), reference)]);
            return Ok(());
        }
        // Computed before the `$ref` is rewritten, since it follows the ref.
        let hidden = match hiding {
            Hiding::Off => None,
            Hiding::On(inherited) => {
                let mut hidden = self.hidden_names(map)?;
                hidden.extend(inherited.iter().cloned());
                Some(hidden)
            }
        };
        if let Some(target) = target {
            let reference = self.def_ref(target, mode);
            map.insert("$ref".to_owned(), reference);
        }
        match self.document.version() {
            SpecVersion::V3_0 => normalize_3_0(map),
            SpecVersion::V3_1 => {}
        }
        if let Some(hidden) = hidden.as_ref().filter(|hidden| !hidden.is_empty()) {
            if let Some(Value::Object(properties)) = map.get_mut("properties") {
                properties.retain(|name, _| !hidden.contains(name));
            }
            if let Some(Value::Array(required)) = map.get_mut("required") {
                required.retain(|name| !name.as_str().is_some_and(|name| hidden.contains(name)));
            }
        }
        let empty = BTreeSet::new();
        for_each_subschema_mut(map, |keyword, subschema| {
            let child = match (hidden.as_ref(), keyword) {
                // Removing constraints under these would invert or shift what
                // they mean, so they pass through unhidden.
                (None, _) | (_, "not" | "if" | "contains") => Hiding::Off,
                // `allOf` branches describe the same instance as their parent.
                (Some(hidden), "allOf") => Hiding::On(hidden),
                (Some(_), _) => Hiding::On(&empty),
            };
            self.transform(subschema, child)
        })
    }

    /// Names of properties `schema` hides in this direction. Its `$ref` and
    /// `allOf` branches describe the same instance, so their hidden names
    /// count too. Walks a worklist, so a long ref chain cannot overflow the
    /// stack.
    fn hidden_names(&self, schema: &Map<String, Value>) -> Result<BTreeSet<String>, RefError> {
        let mut hidden = BTreeSet::new();
        let mut visited = HashSet::new();
        let mut pending = vec![schema];
        while let Some(schema) = pending.pop() {
            if let Some(target) = self.document.schema_ref_target(schema)? {
                if visit(&mut visited, &target)? {
                    if let Some(Value::Object(target)) = self.document.get(&target) {
                        pending.push(target);
                    }
                }
                if self.document.version().ignores_ref_siblings() {
                    continue;
                }
            }
            if let Some(Value::Object(properties)) = schema.get("properties") {
                for (name, property) in properties {
                    if self.marks_hidden(property)? {
                        hidden.insert(name.clone());
                    }
                }
            }
            pending.extend(branches(schema, "allOf").filter_map(Value::as_object));
        }
        Ok(hidden)
    }

    /// Whether `schema` carries this direction's hidden keyword, directly or
    /// through its `$ref` or an `allOf` branch, as opencode does. A flag on
    /// one `anyOf` or `oneOf` branch says nothing about the others. The
    /// schema's own flag counts even beside a 3.0 `$ref`, where generators
    /// put it.
    fn marks_hidden(&self, schema: &Value) -> Result<bool, RefError> {
        let keyword = self.direction.hidden_keyword();
        let mut visited = HashSet::new();
        let mut pending = vec![schema];
        while let Some(schema) = pending.pop() {
            let Value::Object(map) = schema else {
                continue;
            };
            if map.get(keyword) == Some(&Value::Bool(true)) {
                return Ok(true);
            }
            if let Some(target) = self.document.schema_ref_target(map)? {
                if visit(&mut visited, &target)? {
                    if let Some(target) = self.document.get(&target) {
                        pending.push(target);
                    }
                }
                if self.document.version().ignores_ref_siblings() {
                    continue;
                }
            }
            pending.extend(branches(map, "allOf"));
        }
        Ok(false)
    }

    /// The `$ref` value pointing at `target`'s entry in `$defs`.
    fn def_ref(&mut self, target: JsonPointer, mode: DefMode) -> Value {
        // Keys never contain `~` or `/`, so they need no pointer escaping.
        Value::String(format!("#/$defs/{}", self.def_key(target, mode)))
    }

    /// A stable, readable `$defs` key for `target` in `mode`, queued on first
    /// sight.
    fn def_key(&mut self, target: JsonPointer, mode: DefMode) -> String {
        if let Some(key) = self.def_keys.get(&(target.clone(), mode)) {
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
        self.def_keys.insert((target.clone(), mode), key.clone());
        self.pending.push((target, mode, key.clone()));
        key
    }
}

/// Whether a schema hides properties for this direction.
#[derive(Clone, Copy)]
enum Hiding<'a> {
    /// Under `not`, `if`, or `contains`, where removing a property would
    /// change what the schema means.
    Off,
    /// Hide this schema's own hidden properties plus `inherited`, the names an
    /// enclosing `allOf` hides.
    On(&'a BTreeSet<String>),
}

/// How a `$defs` entry is rendered. A target referenced both under `not` and
/// elsewhere gets one entry per mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DefMode {
    /// Hidden properties removed, for ordinary use sites.
    Hiding,
    /// Unchanged, for use under `not`, `if`, or `contains`.
    Verbatim,
}

/// Record `target` as walked, returning whether it is new. Fails once a walk
/// passes `MAX_ALLOF_REFS` schemas.
fn visit(visited: &mut HashSet<JsonPointer>, target: &JsonPointer) -> Result<bool, RefError> {
    if visited.contains(target) {
        return Ok(false);
    }
    if visited.len() == MAX_ALLOF_REFS {
        return Err(RefError::TooManyAllOf {
            reference: target.to_string(),
            limit: MAX_ALLOF_REFS,
        });
    }
    Ok(visited.insert(target.clone()))
}

/// The subschemas of a composition keyword such as `allOf`.
fn branches<'a>(schema: &'a Map<String, Value>, keyword: &str) -> impl Iterator<Item = &'a Value> {
    schema
        .get(keyword)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
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
