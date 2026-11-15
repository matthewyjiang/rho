//! Compiling an Express block into A2UI v1.0 messages.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use super::error::{ExpressError, Position};
use super::parser::{parse, Call, Node, NodeKind, Statement, StatementKind, Target};
use super::signature::Signatures;
use super::{Express, VERSION};
use crate::catalog::Catalog;
use crate::ident::is_identifier;

mod component;
mod value;

pub(super) use component::default_check_message;

/// The highest list index a data path assignment may name.
const MAX_LIST_INDEX: usize = 10_000;

/// How deep values may nest once variables are substituted.
const MAX_VALUE_DEPTH: usize = 256;

/// How many values one block may compile to once variables are
/// substituted, which bounds a block whose variables refer to each other
/// many times over.
const MAX_VALUES: usize = 100_000;

/// The keyword arguments `surface(...)` takes.
const SURFACE_KEYWORDS: [&str; 5] = [
    "surfaceId",
    "catalogId",
    "sendDataModel",
    "update",
    "extensions",
];

impl Express {
    /// Compiles one Express block into A2UI v1.0 messages, in statement
    /// order. A block without a `surface(...)` line targets `surface_id`.
    ///
    /// The surface a block creates or updates is its own business: a block
    /// that assigns `root` creates its surface, one that does not updates a
    /// surface that already exists. Every component compiled is checked
    /// against its catalog's schema.
    ///
    /// # Errors
    ///
    /// An [`ErrorKind::Parse`](super::ErrorKind::Parse) error for text that
    /// is not Express, and an
    /// [`ErrorKind::Validation`](super::ErrorKind::Validation) error for a
    /// block the catalogs refuse. Either carries the position of the
    /// construct at fault where there is one.
    pub fn compile(&self, block: &str, surface_id: &str) -> Result<Vec<Value>, ExpressError> {
        let statements = parse(block)?;
        let units = self.units(&statements, surface_id)?;
        if units.is_empty() {
            return Err(ExpressError::parse(
                Position { line: 1, column: 1 },
                "the block does nothing: assign `root = ...` to create a surface, or write a \
                 component, data path, surface or function call statement",
            ));
        }
        let mut compiler = Compiler {
            express: self,
            calls: 0,
            scope: Scope::empty(),
            state: ScopeState::default(),
        };
        let mut messages = Vec::new();
        for unit in units {
            match unit {
                Unit::Delete(surface_id) => {
                    messages.push(
                        json!({ "version": VERSION, "deleteSurface": { "surfaceId": surface_id } }),
                    );
                }
                Unit::Call { call, pos, scope } => {
                    compiler.scope = scope;
                    let message = compiler.standalone_call(call).map_err(|e| e.at(pos))?;
                    messages.push(message);
                }
                Unit::Scope(scope) => {
                    let pos = scope.pos;
                    compiler.scope = scope;
                    let scope_messages = compiler.scope_messages().map_err(|e| e.at(pos))?;
                    messages.extend(scope_messages);
                }
            }
        }
        Ok(messages)
    }

    /// Groups statements into surface scopes, deletions and standalone
    /// calls, in statement order.
    fn units<'a>(
        &self,
        statements: &'a [Statement],
        surface_id: &str,
    ) -> Result<Vec<Unit<'a>>, ExpressError> {
        let mut units: Vec<Unit<'a>> = Vec::new();
        let mut current: Option<usize> = None;
        for statement in statements {
            match &statement.kind {
                StatementKind::Expr(_) => {
                    let Some(call) = statement.call() else {
                        continue;
                    };
                    match call.name.as_str() {
                        "surface" => {
                            let scope = self
                                .open_scope(call, surface_id)
                                .map_err(|e| e.at(statement.pos))?;
                            current = Some(units.len());
                            units.push(Unit::Scope(Scope {
                                pos: statement.pos,
                                ..scope
                            }));
                        }
                        "deleteSurface" => {
                            let target = string_arg(call, 0, "surfaceId")
                                .map_err(|e| e.at(statement.pos))?
                                .ok_or_else(|| {
                                    ExpressError::validation(
                                        "deleteSurface() needs the id of the surface to delete",
                                    )
                                    .at(statement.pos)
                                })?;
                            units.push(Unit::Delete(target.to_owned()));
                        }
                        _ => units.push(Unit::Call {
                            call,
                            pos: statement.pos,
                            scope: Scope::empty(),
                        }),
                    }
                }
                StatementKind::Assign { target, value } => {
                    let index = if let Some(index) = current {
                        index
                    } else {
                        units.push(Unit::Scope(Scope {
                            surface_id: surface_id.to_owned(),
                            pos: statement.pos,
                            ..Scope::empty()
                        }));
                        units.len() - 1
                    };
                    current = Some(index);
                    if let Unit::Scope(scope) = &mut units[index] {
                        scope.assign(target, value, statement.pos);
                    }
                }
            }
        }
        // A standalone call sees every variable its scope assigns, before or
        // after it.
        let finished: Vec<Option<Symbols<'a>>> = units
            .iter()
            .map(|unit| match unit {
                Unit::Scope(scope) => Some(scope.symbols.clone()),
                _ => None,
            })
            .collect();
        let mut owner: Option<usize> = None;
        for (index, unit) in units.iter_mut().enumerate() {
            match unit {
                Unit::Scope(_) => owner = Some(index),
                Unit::Call { scope, .. } => {
                    if let Some(symbols) = owner.and_then(|i| finished[i].clone()) {
                        scope.symbols = symbols;
                    }
                }
                Unit::Delete(_) => {}
            }
        }
        Ok(units)
    }

    /// The scope a `surface(...)` statement opens.
    fn open_scope<'a>(
        &self,
        call: &'a Call,
        default_surface: &str,
    ) -> Result<Scope<'a>, ExpressError> {
        if call.args.len() > 2 {
            return Err(ExpressError::validation(
                "surface() takes at most two positional arguments: the surface id and the catalog id",
            ));
        }
        if let Some((key, _)) = call
            .kwargs
            .iter()
            .find(|(key, _)| !SURFACE_KEYWORDS.contains(&key.as_str()))
        {
            return Err(ExpressError::validation(format!(
                "surface() has no argument {key}; it takes {}",
                SURFACE_KEYWORDS.join(", ")
            )));
        }
        let surface_id = string_arg(call, 0, "surfaceId")?.unwrap_or(default_surface);
        if let Some(catalog_id) = string_arg(call, 1, "catalogId")? {
            if self.is_multi_catalog() {
                return Err(ExpressError::validation(format!(
                    "surface() names catalog {catalog_id:?}, but a surface names no catalog when \
                     several catalogs are active; put catalogId only on components and calls whose \
                     name several catalogs define"
                )));
            }
            self.catalog_for(catalog_id)?;
        }
        let send_data_model = bool_kwarg(call, "sendDataModel")?;
        let update = bool_kwarg(call, "update")?.unwrap_or(false);
        let extensions = call.kwarg("extensions");
        if update && extensions.is_some() {
            return Err(ExpressError::validation(
                "surface() with update=true does not accept extensions",
            ));
        }
        Ok(Scope {
            surface_id: surface_id.to_owned(),
            send_data_model,
            update,
            extensions,
            ..Scope::empty()
        })
    }

    /// The index of the active catalog with id `id`.
    pub(super) fn catalog_for(&self, id: &str) -> Result<usize, ExpressError> {
        self.catalog_index(id).ok_or_else(|| {
            let ids: Vec<&str> = self.catalogs.iter().map(Catalog::id).collect();
            ExpressError::validation(format!(
                "unknown catalog {id:?}; the active catalogs are {}",
                ids.join(", ")
            ))
        })
    }

    /// Whether a call to `name` names a component: of the catalog
    /// `catalog_id` names when it names an active one, else of any catalog.
    fn defines_component(&self, name: &str, catalog_id: Option<&Node>) -> bool {
        match catalog_id
            .and_then(Node::as_str)
            .and_then(|id| self.catalog_index(id))
        {
            Some(index) => self.signatures[index].components.contains_key(name),
            None => self
                .signatures
                .iter()
                .any(|s| s.components.contains_key(name)),
        }
    }

    /// Whether a call to `name` names a function.
    fn defines_function(&self, name: &str, catalog_id: Option<&Node>) -> bool {
        if name.starts_with('@') {
            return true;
        }
        match catalog_id {
            Some(node) => node
                .as_str()
                .and_then(|id| self.catalog_index(id))
                .is_some_and(|index| self.signatures[index].functions.contains_key(name)),
            None => self
                .signatures
                .iter()
                .any(|s| s.functions.contains_key(name)),
        }
    }

    /// The catalog a component or function call resolves against, and the
    /// catalog id to write on the compiled value: the explicit one, else
    /// the resolved one when several catalogs are active.
    fn resolve(
        &self,
        kind: Kind,
        name: &str,
        catalog_id: Option<&Node>,
    ) -> Result<(usize, Option<String>), ExpressError> {
        let defines = |index: usize| match kind {
            Kind::Component => self.signatures[index].components.contains_key(name),
            Kind::Function | Kind::Check => self.signatures[index].functions.contains_key(name),
        };
        let label = kind.label();
        if let Some(node) = catalog_id {
            let Some(id) = node.as_str() else {
                return Err(ExpressError::validation(format!(
                    "the catalogId of {label} {name} must be a string, not {}",
                    node.describe()
                ))
                .at(node.pos));
            };
            let index = self.catalog_for(id).map_err(|e| e.at(node.pos))?;
            if !defines(index) {
                return Err(ExpressError::validation(format!(
                    "unknown {label} {name}: catalog {id:?} does not define it"
                )));
            }
            return Ok((index, Some(id.to_owned())));
        }
        let matches: Vec<usize> = (0..self.catalogs.len()).filter(|&i| defines(i)).collect();
        match matches.as_slice() {
            [index] => Ok((
                *index,
                self.is_multi_catalog()
                    .then(|| self.catalogs[*index].id().to_owned()),
            )),
            [] => Err(ExpressError::validation(self.unknown_message(kind, name))),
            several => {
                let ids: Vec<&str> = several.iter().map(|&i| self.catalogs[i].id()).collect();
                Err(ExpressError::validation(format!(
                    "{label} {name} is defined in several catalogs ({}); say which with a catalogId argument",
                    ids.join(", ")
                )))
            }
        }
    }

    fn unknown_message(&self, kind: Kind, name: &str) -> String {
        let known: Vec<&str> = self
            .signatures
            .iter()
            .flat_map(|s| match kind {
                Kind::Component => s.component_order.iter(),
                Kind::Function | Kind::Check => s.function_order.iter(),
            })
            .map(String::as_str)
            .collect();
        let label = kind.label();
        if known.is_empty() {
            format!("unknown {label} {name}: the catalogs define none")
        } else {
            format!(
                "unknown {label} {name}; the catalogs define {}",
                known.join(", ")
            )
        }
    }
}

/// What a name is looked up as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Component,
    Function,
    Check,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Component => "component",
            Self::Function => "function",
            Self::Check => "check function",
        }
    }
}

/// The variables of a scope, in the order first assigned; a later
/// assignment replaces the value in place.
#[derive(Debug, Clone, Default)]
struct Symbols<'a> {
    order: Vec<(&'a str, &'a Node)>,
    index: HashMap<&'a str, usize>,
}

impl<'a> Symbols<'a> {
    fn get(&self, name: &str) -> Option<&'a Node> {
        self.index.get(name).map(|&i| self.order[i].1)
    }

    fn set(&mut self, name: &'a str, value: &'a Node) {
        if let Some(&i) = self.index.get(name) {
            self.order[i].1 = value;
        } else {
            self.index.insert(name, self.order.len());
            self.order.push((name, value));
        }
    }
}

/// The statements after a `surface(...)` line, or the implicit scope of a
/// block without one.
#[derive(Debug, Clone)]
struct Scope<'a> {
    surface_id: String,
    pos: Position,
    send_data_model: Option<bool>,
    update: bool,
    extensions: Option<&'a Node>,
    symbols: Symbols<'a>,
    /// Data path assignments: the path as written, after the `$`, in the
    /// order last assigned.
    data: Vec<(&'a str, &'a Node)>,
}

impl<'a> Scope<'a> {
    fn empty() -> Self {
        Self {
            surface_id: String::new(),
            pos: Position { line: 1, column: 1 },
            send_data_model: None,
            update: false,
            extensions: None,
            symbols: Symbols::default(),
            data: Vec::new(),
        }
    }

    fn assign(&mut self, target: &'a Target, value: &'a Node, _pos: Position) {
        match target {
            Target::Variable(name) => self.symbols.set(name, value),
            Target::Path(path) => {
                self.data.retain(|(assigned, _)| *assigned != path);
                self.data.push((path, value));
            }
        }
    }
}

enum Unit<'a> {
    Scope(Scope<'a>),
    Delete(String),
    Call {
        call: &'a Call,
        pos: Position,
        scope: Scope<'a>,
    },
}

/// What a block's compilation remembers from one scope to the next.
struct Compiler<'e, 'a> {
    express: &'e Express,
    /// Standalone calls compiled so far, which numbers their call ids.
    calls: usize,
    scope: Scope<'a>,
    state: ScopeState,
}

/// What compiling one scope keeps track of.
#[derive(Default)]
struct ScopeState {
    /// Components hoisted out of the component being compiled, in order.
    extras: Vec<Value>,
    inline_counter: usize,
    used_ids: HashSet<String>,
    /// Variables whose values are being compiled, to catch a cycle.
    resolving: Vec<String>,
    /// The bound value of the component whose checks are being compiled.
    value_path: Option<Value>,
    /// Where each component's definition starts, for validation errors.
    positions: HashMap<String, Position>,
    depth: usize,
    values: usize,
}

/// Where a value is being compiled: the slot's schema and what it admits.
#[derive(Debug, Clone, Default)]
struct Slot<'s> {
    /// Whether the value sits in an action, so a call is a `functionCall`.
    action: bool,
    /// The component and property an inline component is hoisted from.
    parent: Option<(&'s str, &'s str)>,
    index: Option<usize>,
    /// Whether an unassigned variable names a component from an earlier
    /// turn.
    component_ref: bool,
    /// Whether the items of a list here name components.
    list_refs: bool,
    /// The schema declared here, with the catalog that declares it.
    schema: Option<(Value, usize)>,
}

impl Slot<'_> {
    fn plain() -> Self {
        Self::default()
    }
}

impl<'e, 'a> Compiler<'e, 'a> {
    fn signatures(&self, index: usize) -> &'e Signatures {
        &self.express.signatures[index]
    }

    /// The messages of one scope.
    fn scope_messages(&mut self) -> Result<Vec<Value>, ExpressError> {
        self.state = ScopeState::default();
        self.seed_component_ids()?;
        let mut components = Vec::new();
        let symbols = self.scope.symbols.order.clone();
        for (name, node) in symbols {
            let compiled = self
                .variable_component(name, node)
                .map_err(|e| e.at(node.pos))?;
            if let Some(component) = compiled {
                components.push(component);
                components.append(&mut self.state.extras);
            }
        }
        self.validate_components(&components)?;
        let surface_id = self.scope.surface_id.clone();
        let creates = !self.scope.update && components.iter().any(|c| c["id"] == "root");
        if creates {
            return self.create_message(surface_id, components).map(|m| vec![m]);
        }
        if self.scope.extensions.is_some() {
            return Err(ExpressError::validation(
                "surface() extensions need a scope that creates the surface, with a root component",
            ));
        }
        let mut messages = Vec::new();
        let data = self.scope.data.clone();
        for (path, node) in data {
            let value = self.data_value(node)?;
            let pointer = format!("/{}", strip_path(path));
            if pointer == "/" && !value.is_object() {
                return Err(ExpressError::validation(format!(
                    "the root data path $/ takes a map, not {}",
                    node.describe()
                ))
                .at(node.pos));
            }
            messages.push(json!({
                "version": VERSION,
                "updateDataModel": { "surfaceId": surface_id, "path": pointer, "value": value },
            }));
        }
        if components.is_empty() {
            if messages.is_empty() {
                return Err(ExpressError::parse(
                    self.scope.pos,
                    "the surface scope assigns no component and no data path; assign `root = ...` \
                     to create the surface",
                ));
            }
            return Ok(messages);
        }
        messages.insert(
            0,
            json!({
                "version": VERSION,
                "updateComponents": { "surfaceId": surface_id, "components": components },
            }),
        );
        Ok(messages)
    }

    fn create_message(
        &mut self,
        surface_id: String,
        components: Vec<Value>,
    ) -> Result<Value, ExpressError> {
        let mut data_model = Map::new();
        let data = self.scope.data.clone();
        for (path, node) in data {
            let value = self.data_value(node)?;
            set_path(&mut data_model, path, value).map_err(|e| e.at(node.pos))?;
        }
        let mut create = Map::new();
        create.insert("surfaceId".to_owned(), Value::String(surface_id));
        if !self.express.is_multi_catalog() {
            create.insert(
                "catalogId".to_owned(),
                Value::from(self.express.catalogs[0].id()),
            );
        }
        if let Some(flag) = self.scope.send_data_model {
            create.insert("sendDataModel".to_owned(), Value::Bool(flag));
        }
        if let Some(node) = self.scope.extensions {
            let extensions = self.extensions(node, "surface()")?;
            create.insert("metadata".to_owned(), json!({ "extensions": extensions }));
        }
        create.insert("components".to_owned(), Value::Array(components));
        if !data_model.is_empty() {
            create.insert("dataModel".to_owned(), Value::Object(data_model));
        }
        Ok(json!({ "version": VERSION, "createSurface": create }))
    }

    fn data_value(&mut self, node: &'a Node) -> Result<Value, ExpressError> {
        self.value(node, &Slot::plain())
    }

    /// Checks each component against its catalog's schema.
    fn validate_components(&self, components: &[Value]) -> Result<(), ExpressError> {
        for component in components {
            let catalog = match component.get("@catalogId").and_then(Value::as_str) {
                Some(id) => self.express.catalog_index(id).unwrap_or(0),
                None => 0,
            };
            if let Err(error) = self.express.validators[catalog].validate_component(component) {
                let id = component["id"].as_str().unwrap_or_default();
                let name = component["component"].as_str().unwrap_or_default();
                let error = ExpressError::validation(format!(
                    "component {id} ({name}) does not match its catalog schema: {error}"
                ));
                return Err(match self.state.positions.get(id) {
                    Some(&pos) => error.at(pos),
                    None => error,
                });
            }
        }
        Ok(())
    }

    /// Reserves the id of every component a variable holds, so an inline
    /// component cannot take one.
    fn seed_component_ids(&mut self) -> Result<(), ExpressError> {
        for &(name, node) in &self.scope.symbols.order {
            let NodeKind::Call(call) = &node.kind else {
                continue;
            };
            if self
                .express
                .defines_component(&call.name, call.kwarg("catalogId"))
            {
                let id = component_id(name, call).map_err(|e| e.at(node.pos))?;
                if !self.state.used_ids.insert(id.clone()) {
                    return Err(
                        ExpressError::validation(format!("duplicate component id {id:?}"))
                            .at(node.pos),
                    );
                }
                self.state.positions.insert(id, node.pos);
            }
        }
        Ok(())
    }

    /// The component a variable holds, or `None` when it holds a value,
    /// which is substituted wherever it is named.
    fn variable_component(
        &mut self,
        name: &str,
        node: &'a Node,
    ) -> Result<Option<Value>, ExpressError> {
        let NodeKind::Call(call) = &node.kind else {
            return Ok(None);
        };
        if call.name.starts_with('@') {
            self.system_function(call)?;
            return Ok(None);
        }
        let catalog_id = call.kwarg("catalogId");
        let reserved = matches!(call.name.as_str(), "Event" | "_template") && catalog_id.is_none();
        if !self.express.defines_component(&call.name, catalog_id)
            && (reserved || self.express.defines_function(&call.name, catalog_id))
        {
            return Ok(None);
        }
        self.component(name, call).map(Some)
    }
}

/// The id of the component `call` compiles to under variable `name`: the
/// variable name, unless the call sets `id=`.
fn component_id(name: &str, call: &Call) -> Result<String, ExpressError> {
    let Some(node) = call.kwarg("id") else {
        return Ok(name.to_owned());
    };
    match node.as_str() {
        Some(id) if is_identifier(id) => Ok(id.to_owned()),
        Some(id) => Err(ExpressError::validation(format!(
            "component id {id:?} is not a UAX #31 identifier, as A2UI v1.0 requires"
        ))
        .at(node.pos)),
        None => Err(ExpressError::validation(format!(
            "a component id must be a string, not {}",
            node.describe()
        ))
        .at(node.pos)),
    }
}

/// A string argument of `surface` or `deleteSurface`, or `None` when absent.
fn string_arg<'n>(
    call: &'n Call,
    index: usize,
    key: &str,
) -> Result<Option<&'n str>, ExpressError> {
    let Some(node) = call.arg(index, key) else {
        return Ok(None);
    };
    match node.as_str() {
        Some(text) => Ok(Some(text)),
        None => Err(ExpressError::validation(format!(
            "{}() argument {key} must be a string, not {}",
            call.name,
            node.describe()
        ))
        .at(node.pos)),
    }
}

fn bool_kwarg(call: &Call, key: &str) -> Result<Option<bool>, ExpressError> {
    let Some(node) = call.kwarg(key) else {
        return Ok(None);
    };
    match &node.kind {
        NodeKind::Literal(Value::Bool(flag)) => Ok(Some(*flag)),
        _ => Err(ExpressError::validation(format!(
            "{key} must be true or false, not {}",
            node.describe()
        ))
        .at(node.pos)),
    }
}

/// The path after the `$`, without its leading `/`.
fn strip_path(path: &str) -> &str {
    path.strip_prefix('/').unwrap_or(path)
}

/// Sets `value` at the data path `path` of `model`, creating maps and lists
/// on the way: a missing segment followed by a list index starts a list,
/// and setting past the end of a list pads it with `null`.
fn set_path(model: &mut Map<String, Value>, path: &str, value: Value) -> Result<(), ExpressError> {
    let clean = strip_path(path);
    if clean.is_empty() {
        let Value::Object(map) = value else {
            return Err(ExpressError::validation(format!(
                "the root data path ${path} takes a map"
            )));
        };
        *model = map;
        return Ok(());
    }
    let keys: Vec<&str> = clean.split('/').collect();
    let mut root = Value::Object(std::mem::take(model));
    let result = set_in(&mut root, &keys, path, value);
    if let Value::Object(map) = root {
        *model = map;
    }
    result
}

fn set_in(root: &mut Value, keys: &[&str], path: &str, value: Value) -> Result<(), ExpressError> {
    let (last, parents) = keys.split_last().expect("a data path has a segment");
    let mut cursor = root;
    for (i, key) in parents.iter().enumerate() {
        cursor = step(cursor, key, keys[i + 1], path)?;
    }
    match cursor {
        Value::Object(map) => {
            map.insert((*last).to_owned(), value);
            Ok(())
        }
        Value::Array(items) => {
            let index = list_index(last, path)?;
            if items.len() <= index {
                items.resize(index + 1, Value::Null);
            }
            items[index] = value;
            Ok(())
        }
        _ => Err(ExpressError::validation(format!(
            "cannot set the data path ${path}: a segment on the way holds a plain value"
        ))),
    }
}

/// One step down a data path, creating the container if it is missing: a
/// list when the next segment is a list index, else a map.
fn step<'v>(
    current: &'v mut Value,
    key: &str,
    next: &str,
    path: &str,
) -> Result<&'v mut Value, ExpressError> {
    let fresh = || {
        if is_list_index(next) {
            Value::Array(Vec::new())
        } else {
            Value::Object(Map::new())
        }
    };
    let slot = match current {
        Value::Object(map) => map.entry(key.to_owned()).or_insert(Value::Null),
        Value::Array(items) => {
            let index = list_index(key, path)?;
            if items.len() <= index {
                items.resize(index + 1, Value::Null);
            }
            &mut items[index]
        }
        _ => {
            return Err(ExpressError::validation(format!(
                "cannot set the data path ${path}: segment {key:?} holds a plain value"
            )))
        }
    };
    if slot.is_null() {
        *slot = fresh();
    }
    if slot.is_object() || slot.is_array() {
        Ok(slot)
    } else {
        Err(ExpressError::validation(format!(
            "cannot set the data path ${path}: segment {key:?} holds a plain value"
        )))
    }
}

fn is_list_index(segment: &str) -> bool {
    segment == "0"
        || (!segment.starts_with('0')
            && !segment.is_empty()
            && segment.bytes().all(|b| b.is_ascii_digit()))
}

fn list_index(segment: &str, path: &str) -> Result<usize, ExpressError> {
    if !is_list_index(segment) {
        return Err(ExpressError::validation(format!(
            "the data path ${path} names {segment:?} in a list, which takes an index"
        )));
    }
    match segment.parse::<usize>() {
        Ok(index) if index <= MAX_LIST_INDEX => Ok(index),
        _ => Err(ExpressError::validation(format!(
            "the data path ${path} names list index {segment}, past the highest, {MAX_LIST_INDEX}"
        ))),
    }
}

#[cfg(test)]
#[path = "compile_tests.rs"]
mod tests;
