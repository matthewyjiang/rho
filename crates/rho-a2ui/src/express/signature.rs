//! What Express needs to know of a catalog: each component's and function's
//! parameters in positional order, and what kind of value each slot takes.
//! Positions follow the catalog document: a component's own properties
//! first, then those of the mixins it composes, and `checks` last.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::catalog::{Callers, Catalog, EntryKind, FunctionDef, ReturnType};
use crate::schema;

/// The bare names of the component envelope mixins. From v1.0 they travel
/// with an `@`: `@catalogId`, `@metadata`, `@accessibility`, `@checks`.
const ENVELOPE_NAMES: [&str; 4] = ["catalogId", "metadata", "accessibility", "checks"];

/// `ComponentCommon`'s description of `catalogId`, which marks a bare
/// `catalogId` property as the mixin's rather than the catalog's own.
const CATALOG_ID_DESCRIPTION: &str =
    "The catalog ID for this component, overriding any surface-level default catalogId.";

/// How deep a schema walk follows references and combinators.
const MAX_SCHEMA_DEPTH: usize = 32;

/// The keywords that constrain a schema's type. A schema with none of them
/// admits any value.
const TYPE_KEYS: [&str; 11] = [
    "type",
    "const",
    "enum",
    "$ref",
    "allOf",
    "oneOf",
    "anyOf",
    "properties",
    "items",
    "prefixItems",
    "not",
];

/// The signatures of one catalog.
#[derive(Debug, Clone)]
pub(crate) struct Signatures {
    pub(crate) components: BTreeMap<String, ComponentSignature>,
    /// Component names in catalog order.
    pub(crate) component_order: Vec<String>,
    pub(crate) functions: BTreeMap<String, FunctionSignature>,
    /// Function names in catalog order.
    pub(crate) function_order: Vec<String>,
    /// The catalog document with its local references written inline, which
    /// slot schemas resolve their own `#/` references against.
    document: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ComponentSignature {
    /// The parameters in positional order. A checkable component's last one
    /// is `checks`.
    pub(crate) params: Vec<String>,
    pub(crate) required: Vec<String>,
    pub(crate) checkable: bool,
    /// Envelope mixin names the component declares as ordinary properties
    /// of its own, which therefore travel bare.
    pub(crate) custom_envelope: BTreeSet<String>,
    pub(crate) description: Option<String>,
    /// Each property's schema: the component's own definition wins over its
    /// mixins'.
    schemas: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct FunctionSignature {
    pub(crate) params: Vec<String>,
    pub(crate) required: Vec<String>,
    pub(crate) description: Option<String>,
    pub(crate) callers: Callers,
    pub(crate) requires_user_activation: bool,
    pub(crate) return_type: ReturnType,
    schemas: BTreeMap<String, Value>,
}

/// What a value slot holds, read from the `$ref`s of its schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotKind {
    /// One child component, by id.
    Child,
    /// Children: a list of ids, or a template.
    ChildList,
    /// An action: an event or a renderer function call.
    Action,
}

impl Signatures {
    pub(crate) fn new(catalog: &Catalog) -> Self {
        let raw = catalog.to_json();
        let document = inline_local_refs(&raw, &raw, &mut Vec::new());
        let entries = |kind: &str| document.get(kind).and_then(Value::as_object);
        let components = entries("components")
            .into_iter()
            .flatten()
            .map(|(name, schema)| (name.clone(), ComponentSignature::new(schema)))
            .collect();
        let functions = entries("functions")
            .into_iter()
            .flatten()
            .map(|(name, schema)| {
                (
                    name.clone(),
                    FunctionSignature::new(schema, catalog.function(name)),
                )
            })
            .collect();
        Self {
            components,
            component_order: names(catalog, EntryKind::Component),
            functions,
            function_order: names(catalog, EntryKind::Function),
            document,
        }
    }

    /// The schema at the items of an array schema, following references.
    pub(crate) fn item_schema(&self, schema: &Value) -> Option<Value> {
        self.resolve_at(schema, None, Home::Catalog, 0)
    }

    /// The schema of property `name` of an object schema, following
    /// references, else its `additionalProperties`.
    pub(crate) fn property_schema(&self, schema: &Value, name: &str) -> Option<Value> {
        self.resolve_at(schema, Some(name), Home::Catalog, 0)
    }

    fn resolve_at(
        &self,
        schema: &Value,
        prop: Option<&str>,
        home: Home,
        depth: usize,
    ) -> Option<Value> {
        let map = schema.as_object()?;
        if depth > MAX_SCHEMA_DEPTH {
            return None;
        }
        match prop {
            None => {
                if let Some(items) = map.get("items").filter(|items| items.is_object()) {
                    return Some(items.clone());
                }
            }
            Some(name) => {
                let declared = map.get("properties").and_then(|p| p.get(name));
                if let Some(declared) = declared.filter(|d| d.is_object()) {
                    return Some(declared.clone());
                }
                if let Some(extra) = map.get("additionalProperties").filter(|a| a.is_object()) {
                    return Some(extra.clone());
                }
            }
        }
        if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
            if let Some((target, home)) = self.resolve_ref(reference, home) {
                if let Some(found) = self.resolve_at(target, prop, home, depth + 1) {
                    return Some(found);
                }
            }
        }
        combinators(map).find_map(|sub| self.resolve_at(sub, prop, home, depth + 1))
    }

    /// The target of a `$ref` met in a schema that lives in `home`.
    fn resolve_ref<'a>(&'a self, reference: &str, home: Home) -> Option<(&'a Value, Home)> {
        if let Some(name) = common_def_name(reference) {
            return Some((schema::common_type(name)?, Home::CommonTypes));
        }
        if home == Home::CommonTypes {
            if let Some(name) = reference.strip_prefix("#/$defs/") {
                return Some((schema::common_type(name)?, Home::CommonTypes));
            }
        }
        let pointer = reference
            .strip_prefix("catalog.json#")
            .or_else(|| reference.strip_prefix('#'))?;
        Some((self.document.pointer(pointer)?, Home::Catalog))
    }

    /// Whether a schema admits a number, following references. An
    /// unresolvable reference or an untyped schema admits one.
    pub(crate) fn accepts_number(&self, schema: &Value) -> bool {
        self.accepts_number_in(schema, Home::Catalog, 0)
    }

    fn accepts_number_in(&self, schema: &Value, home: Home, depth: usize) -> bool {
        let Some(map) = schema.as_object() else {
            return false;
        };
        if depth > MAX_SCHEMA_DEPTH {
            return false;
        }
        let numeric = |t: &Value| matches!(t.as_str(), Some("number" | "integer"));
        match map.get("type") {
            Some(Value::Array(types)) if types.iter().any(numeric) => return true,
            Some(t) if numeric(t) => return true,
            _ => {}
        }
        if map.get("const").is_some_and(Value::is_number) {
            return true;
        }
        if map
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().any(Value::is_number))
        {
            return true;
        }
        if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
            match self.resolve_ref(reference, home) {
                Some((target, home)) => {
                    if self.accepts_number_in(target, home, depth + 1) {
                        return true;
                    }
                }
                None => return true,
            }
        }
        if combinators(map).any(|sub| self.accepts_number_in(sub, home, depth + 1)) {
            return true;
        }
        !TYPE_KEYS.iter().any(|key| map.contains_key(*key))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Catalog,
    CommonTypes,
}

fn names(catalog: &Catalog, kind: EntryKind) -> Vec<String> {
    catalog
        .ordered_names(kind)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn combinators(map: &Map<String, Value>) -> impl Iterator<Item = &Value> {
    ["allOf", "oneOf", "anyOf"]
        .into_iter()
        .filter_map(|key| map.get(key).and_then(Value::as_array))
        .flatten()
}

/// The common type a `$ref` names, such as `Action` for
/// `common_types.json#/$defs/Action`.
fn common_def_name(reference: &str) -> Option<&str> {
    let (document, fragment) = reference.split_once('#')?;
    if !(document == "common_types.json" || document.ends_with("/common_types.json")) {
        return None;
    }
    let name = fragment
        .strip_prefix("/$defs/")
        .or_else(|| fragment.strip_prefix("/definitions/"))?;
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
}

/// `node` with each local `#/` reference replaced by its target, the keys
/// beside the `$ref` merged over it. A reference to a definition named like
/// a common type stays, as does one a definition makes to itself.
fn inline_local_refs(node: &Value, root: &Value, visiting: &mut Vec<String>) -> Value {
    match node {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
                let name = reference.rsplit('/').next().unwrap_or_default();
                let keep = !reference.starts_with("#/")
                    || (reference.starts_with("#/$defs/") && schema::common_type(name).is_some())
                    || visiting.iter().any(|r| r == reference);
                if !keep {
                    if let Some(Value::Object(target)) = root.pointer(&reference[1..]) {
                        visiting.push(reference.to_owned());
                        let mut merged =
                            match inline_local_refs(&Value::Object(target.clone()), root, visiting)
                            {
                                Value::Object(merged) => merged,
                                _ => Map::new(),
                            };
                        for (key, value) in map {
                            if key != "$ref" {
                                merged
                                    .insert(key.clone(), inline_local_refs(value, root, visiting));
                            }
                        }
                        visiting.pop();
                        return Value::Object(merged);
                    }
                }
            }
            Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), inline_local_refs(value, root, visiting)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| inline_local_refs(item, root, visiting))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A component's sub-schemas, specific definitions (those declaring
/// `component`) before mixins, each group in document order.
fn sub_schemas(schema: &Value) -> Vec<&Map<String, Value>> {
    fn visit<'a>(
        node: &'a Value,
        level: Level,
        specific: &mut Vec<&'a Map<String, Value>>,
        common: &mut Vec<&'a Map<String, Value>>,
    ) {
        let Some(map) = node.as_object() else {
            return;
        };
        let properties = map.get("properties").and_then(Value::as_object);
        if properties.is_some_and(|p| p.contains_key("component")) {
            specific.push(map);
        } else if level == Level::Member
            || properties.is_some()
            || map.contains_key("required")
            || map.contains_key("$ref")
        {
            common.push(map);
        }
        for sub in map
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            visit(sub, Level::Member, specific, common);
        }
    }
    let mut specific = Vec::new();
    let mut common = Vec::new();
    visit(schema, Level::Root, &mut specific, &mut common);
    specific.extend(common);
    specific
}

/// Where a sub-schema sits in a component definition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Root,
    /// A member of an `allOf`, however deep.
    Member,
}

impl ComponentSignature {
    fn new(schema: &Value) -> Self {
        let mut props: Vec<(String, Value)> = Vec::new();
        let mut required: Vec<String> = Vec::new();
        let mut checkable = false;
        for sub in sub_schemas(schema) {
            if sub
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|r| r.contains("Checkable"))
            {
                checkable = true;
            }
            for (name, prop) in sub
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if !props.iter().any(|(seen, _)| seen == name) {
                    props.push((name.clone(), prop.clone()));
                }
            }
            for name in sub
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(name) = name.as_str() {
                    if !required.iter().any(|r| r == name) {
                        required.push(name.to_owned());
                    }
                }
            }
        }
        let schema_of = |name: &str| props.iter().find(|(n, _)| n == name).map(|(_, s)| s);
        if props.iter().any(|(n, _)| n == "@checks")
            || schema_of("checks").is_some_and(is_checks_schema)
        {
            checkable = true;
        }
        let custom_envelope = custom_envelope(schema_of);
        let excluded = |name: &str| {
            matches!(name, "component" | "id")
                || name.starts_with('@')
                || (matches!(name, "catalogId" | "metadata" | "accessibility")
                    && !custom_envelope.contains(name))
                || (name == "checks" && checkable && !custom_envelope.contains("checks"))
        };
        let mut params: Vec<String> = props
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| !excluded(name))
            .collect();
        if checkable && !custom_envelope.contains("checks") {
            params.push("checks".to_owned());
        }
        let own = schema.get("properties").and_then(Value::as_object);
        let schemas = params
            .iter()
            .filter_map(|name| {
                let winning = own
                    .and_then(|own| own.get(name))
                    .or_else(|| schema_of(name))?;
                Some((name.clone(), winning.clone()))
            })
            .collect();
        let description = schema
            .get("description")
            .or_else(|| {
                schema
                    .get("allOf")?
                    .as_array()?
                    .iter()
                    .find_map(|sub| sub.get("description").filter(|d| d.is_string()))
            })
            .and_then(Value::as_str)
            .map(str::to_owned);
        Self {
            params,
            required,
            checkable,
            custom_envelope,
            description,
            schemas,
        }
    }

    /// The schema of parameter `name`.
    pub(crate) fn schema(&self, name: &str) -> Option<&Value> {
        self.schemas.get(name)
    }

    /// Whether the component holds checks in the `@checks` envelope rather
    /// than in a property of its own.
    pub(crate) fn takes_envelope_checks(&self) -> bool {
        self.checkable && !self.custom_envelope.contains("checks")
    }
}

/// The envelope mixin names a component declares as properties of its own,
/// rather than with the mixin's schema.
fn custom_envelope<'s>(schema_of: impl Fn(&str) -> Option<&'s Value>) -> BTreeSet<String> {
    ENVELOPE_NAMES
        .iter()
        .filter(|name| {
            schema_of(name).is_some_and(|schema| match **name {
                "checks" => !is_checks_schema(schema),
                "catalogId" => schema
                    .get("description")
                    .is_none_or(|d| d != CATALOG_ID_DESCRIPTION),
                "metadata" => !is_metadata_schema(schema),
                _ => !is_accessibility_schema(schema),
            })
        })
        .map(|name| (*name).to_owned())
        .collect()
}

fn is_checks_schema(schema: &Value) -> bool {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .is_some_and(|r| r.ends_with("/Checkable"))
        || schema
            .pointer("/items/$ref")
            .and_then(Value::as_str)
            .is_some_and(|r| r.ends_with("/CheckRule"))
}

fn is_metadata_schema(schema: &Value) -> bool {
    let only_extensions = schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.len() == 1 && p.contains_key("extensions"));
    only_extensions && schema.get("additionalProperties") == Some(&Value::Bool(false))
}

fn is_accessibility_schema(schema: &Value) -> bool {
    if schema
        .get("$ref")
        .and_then(Value::as_str)
        .is_some_and(|r| r.ends_with("/AccessibilityAttributes"))
    {
        return true;
    }
    let both = schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key("label") && p.contains_key("description"));
    both && schema.get("additionalProperties") == Some(&Value::Bool(false))
}

impl FunctionSignature {
    /// The signature of a function `schema` defines, with the metadata the
    /// catalog read from it, or none for a system function.
    fn new(schema: &Value, def: Option<&FunctionDef>) -> Self {
        let mut params: Vec<String> = Vec::new();
        let mut required: Vec<String> = Vec::new();
        let mut schemas = BTreeMap::new();
        let members = schema.get("allOf").and_then(Value::as_array);
        for sub in std::iter::once(schema).chain(members.into_iter().flatten()) {
            let Some(args) = sub.pointer("/properties/args") else {
                continue;
            };
            for (name, prop) in args
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                if !params.contains(name) {
                    params.push(name.clone());
                    schemas.insert(name.clone(), prop.clone());
                }
            }
            for name in args
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(name) = name.as_str() {
                    required.push(name.to_owned());
                }
            }
        }
        Self {
            params,
            required,
            description: schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            callers: def.map(FunctionDef::callers).unwrap_or_default(),
            requires_user_activation: def.is_some_and(FunctionDef::requires_user_activation),
            return_type: def.map_or(ReturnType::Any, FunctionDef::return_type),
            schemas,
        }
    }

    /// The schema of parameter `name`.
    pub(crate) fn schema(&self, name: &str) -> Option<&Value> {
        self.schemas.get(name)
    }

    /// Whether the agent may call it with `callRendererFunction`.
    pub(crate) fn agent_callable(&self) -> bool {
        !self.requires_user_activation && self.callers != Callers::RendererOnly
    }
}

/// The protocol's system functions, such as `@index`: the common types
/// whose `@call` is a constant starting with `@`.
pub(crate) fn system_functions() -> BTreeMap<String, FunctionSignature> {
    let defs = schema::common_types()
        .get("$defs")
        .and_then(Value::as_object);
    defs.into_iter()
        .flatten()
        .filter_map(|(_, def)| {
            let name = def.pointer("/properties/@call/const")?.as_str()?;
            name.starts_with('@')
                .then(|| (name.to_owned(), FunctionSignature::new(def, None)))
        })
        .collect()
}

/// The kind of slot a schema declares, from the names of the common types
/// it references.
pub(crate) fn slot_kind(schema: &Value) -> Option<SlotKind> {
    slot_kind_in(schema, 0)
}

fn slot_kind_in(schema: &Value, depth: usize) -> Option<SlotKind> {
    let map = schema.as_object()?;
    if depth > MAX_SCHEMA_DEPTH {
        return None;
    }
    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
        if reference.contains("ChildList") {
            return Some(SlotKind::ChildList);
        }
        if reference.contains("Child") || reference.contains("ComponentId") {
            return Some(SlotKind::Child);
        }
        if reference.contains("Action") {
            return Some(SlotKind::Action);
        }
    }
    combinators(map).find_map(|sub| slot_kind_in(sub, depth + 1))
}

/// Whether a schema is the common `Action` type, directly or through a
/// combinator.
pub(crate) fn is_action(schema: &Value) -> bool {
    let Some(map) = schema.as_object() else {
        return false;
    };
    map.get("$ref")
        .and_then(Value::as_str)
        .and_then(common_def_name)
        == Some("Action")
        || combinators(map).any(is_action)
}

/// Whether a component property admits a data binding: a dynamic common
/// type, or an object with a `path` that is not a child template.
pub(crate) fn allows_binding(schema: &Value) -> bool {
    let Some(map) = schema.as_object() else {
        return false;
    };
    if map
        .get("$ref")
        .and_then(Value::as_str)
        .is_some_and(|r| r.contains("DataBinding") || r.contains("Dynamic"))
    {
        return true;
    }
    if let Some(properties) = map.get("properties").and_then(Value::as_object) {
        if (properties.contains_key("path") || properties.contains_key("@path"))
            && !properties.contains_key("componentId")
        {
            return true;
        }
    }
    map.get("items").is_some_and(allows_binding) || combinators(map).any(allows_binding)
}

/// The first `enum` of a schema or its combinators.
pub(crate) fn find_enum(schema: &Value) -> Option<&Vec<Value>> {
    let map = schema.as_object()?;
    if let Some(Value::Array(values)) = map.get("enum") {
        return Some(values);
    }
    combinators(map)
        .find_map(find_enum)
        .filter(|values| !values.is_empty())
}

/// Whether a schema is a list of `{label, value}` options.
pub(crate) fn expects_options(schema: &Value) -> bool {
    fn has_label_value(schema: &Value) -> bool {
        let Some(map) = schema.as_object() else {
            return false;
        };
        map.get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key("label") && p.contains_key("value"))
            || combinators(map).any(has_label_value)
    }
    let Some(map) = schema.as_object() else {
        return false;
    };
    match map.get("items") {
        Some(items) => has_label_value(items),
        None => combinators(map).any(expects_options),
    }
}
