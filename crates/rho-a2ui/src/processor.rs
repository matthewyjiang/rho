//! The renderer side of A2UI: applying an agent's messages to surfaces,
//! checking each against the surface it joins before anything changes.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Map, Value};

use crate::catalog::{Callers, Catalog, CatalogError, ComponentDef};
use crate::catalogs::{Catalogs, Supported, UnresolvedCatalog};
use crate::component::{self, escape, Component, Envelope, Rejection};
use crate::data_model::{DataError, DataModel, WatchId};
use crate::functions::Functions;
use crate::message::{
    parse_payload, AgentMessage, CallRendererFunction, CreateSurface, ErrorCode, ErrorMessage,
    FunctionFailure, FunctionResponse, RendererMessage, UpdateDataModel, WireComponent,
    PROTOCOL_VERSION,
};
use crate::pointer::Pointer;
use crate::surface::Surface;
use crate::topology::{self, Node, MAX_NESTING};
use crate::validate::Violation;

/// The deepest function calls may nest in a component, from the A2UI core
/// blueprint.
const MAX_CALL_DEPTH: usize = 5;

/// Which checks of a message's content a processor runs, beyond reading the
/// message itself and resolving every catalog it names, which always run.
///
/// Each `allow_` rule relaxes one check, so a renderer can accept a surface
/// that streams in out of order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent rules of the protocol's ValidationConfig"
)]
pub struct ValidationConfig {
    /// Accept components the root does not reach.
    pub allow_orphans: bool,
    /// Accept references to components that have not arrived.
    pub allow_dangling_references: bool,
    /// Accept a surface without a `root` component.
    pub allow_missing_root: bool,
    /// Accept component types, properties and functions the catalog does not
    /// declare. A call to an undeclared function would go to the agent.
    pub allow_unknown_elements: bool,
}

impl ValidationConfig {
    /// Every check on.
    pub const STRICT: Self = Self {
        allow_orphans: false,
        allow_dangling_references: false,
        allow_missing_root: false,
        allow_unknown_elements: false,
    };

    /// Every relaxable check off: schemas, composition and cycles are still
    /// checked.
    pub const RELAXED: Self = Self {
        allow_orphans: true,
        allow_dangling_references: true,
        allow_missing_root: true,
        allow_unknown_elements: true,
    };
}

/// Whether a processor checks content against the catalogs and the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Validation {
    /// Only read messages and resolve catalogs: for a renderer that trusts
    /// its agent.
    Skip,
    /// Also check schemas, composition and topology, by these rules.
    Check(ValidationConfig),
}

/// Something a processed message changed, for a view to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A surface was created.
    SurfaceCreated {
        /// The surface.
        surface: String,
    },
    /// A surface was deleted, with its components and data.
    SurfaceDeleted {
        /// The surface.
        surface: String,
    },
    /// A component was added.
    ComponentAdded {
        /// The surface.
        surface: String,
        /// The component's id.
        component: String,
    },
    /// A component's envelope or properties were replaced, keeping its type
    /// and catalog.
    ComponentUpdated {
        /// The surface.
        surface: String,
        /// The component's id.
        component: String,
    },
    /// A component was removed: its id now names a component of another type
    /// or catalog, which a [`Change::ComponentAdded`] follows.
    ComponentRemoved {
        /// The surface.
        surface: String,
        /// The component's id.
        component: String,
    },
    /// The data model was written at `path`.
    DataUpdated {
        /// The surface.
        surface: String,
        /// Where it was written.
        path: Pointer,
        /// The watchers whose value changed.
        fired: Vec<WatchId>,
    },
}

/// What processing a payload did.
#[derive(Debug, Clone, Default, PartialEq)]
#[must_use]
pub struct Processed {
    /// What changed, in order.
    pub changes: Vec<Change>,
    /// The messages to send the agent: an `error` for each message rejected,
    /// in order, and answers to its calls.
    pub outbound: Vec<RendererMessage>,
}

impl Processed {
    /// The `error` messages among [`Processed::outbound`].
    pub fn errors(&self) -> impl Iterator<Item = &ErrorMessage> {
        self.outbound.iter().filter_map(|message| match message {
            RendererMessage::Error(error) => Some(error),
            _ => None,
        })
    }
}

/// Whether [`MessageProcessor::renderer_capabilities`] lists each catalog's
/// document as well as its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineCatalogs {
    /// Only `supportedCatalogIds`.
    Omit,
    /// `inlineCatalogs` as well.
    Include,
}

/// The surfaces an agent builds, and the catalogs they draw from.
///
/// Messages go in through [`MessageProcessor::process`], which checks each
/// one against the surface it joins and changes nothing for a message it
/// rejects. A view reads the surfaces, resolves a component's properties with
/// [`MessageProcessor::resolve`], and reports a user's action with
/// [`MessageProcessor::dispatch_action`].
#[derive(Debug)]
pub struct MessageProcessor {
    pub(crate) catalogs: Catalogs,
    pub(crate) surfaces: BTreeMap<String, Surface>,
    validation: Validation,
}

impl MessageProcessor {
    /// A processor with no catalogs and no surfaces.
    #[must_use]
    pub fn new(validation: Validation) -> Self {
        Self {
            catalogs: Catalogs::default(),
            surfaces: BTreeMap::new(),
            validation,
        }
    }

    /// Supports `catalog`, whose functions `functions` implements. A function
    /// the catalog declares without an implementation runs elsewhere.
    ///
    /// # Errors
    ///
    /// Fails with [`CatalogError::InvalidSchema`] when a definition does not
    /// compile as a JSON Schema.
    pub fn add_catalog(
        &mut self,
        catalog: Catalog,
        functions: Functions,
    ) -> Result<(), CatalogError> {
        self.catalogs.add(catalog, functions)
    }

    /// The supported catalog `id`.
    #[must_use]
    pub fn catalog(&self, id: &str) -> Option<&Catalog> {
        self.catalogs.get(id).map(|supported| &supported.catalog)
    }

    /// The surface `id`.
    #[must_use]
    pub fn surface(&self, id: &str) -> Option<&Surface> {
        self.surfaces.get(id)
    }

    /// The surface `id`, to write its data model or watch it.
    pub fn surface_mut(&mut self, id: &str) -> Option<&mut Surface> {
        self.surfaces.get_mut(id)
    }

    /// The surfaces, by id.
    pub fn surfaces(&self) -> impl Iterator<Item = &Surface> {
        self.surfaces.values()
    }

    /// The catalog an item that names `catalog_id`, or none, runs in on
    /// surface `surface`.
    ///
    /// # Errors
    ///
    /// Fails when the item names a catalog this processor lacks, or names
    /// none on a surface without a default. An unknown surface has none.
    pub fn resolve_catalog(
        &self,
        surface: &str,
        catalog_id: Option<&str>,
    ) -> Result<&Catalog, UnresolvedCatalog> {
        let default = self
            .surfaces
            .get(surface)
            .and_then(Surface::default_catalog);
        let (_, supported) = self.catalogs.resolve(catalog_id, default, surface)?;
        Ok(&supported.catalog)
    }

    /// Applies a payload: one message, a list, or a `{"messages": [...]}`
    /// wrapper, with the processor's own validation.
    pub fn process(&mut self, payload: &Value) -> Processed {
        self.process_with(payload, self.validation)
    }

    /// Applies a payload with `validation` instead of the processor's own,
    /// for this call only.
    pub fn process_with(&mut self, payload: &Value, validation: Validation) -> Processed {
        let mut processed = Processed::default();
        for message in parse_payload(payload) {
            match message.and_then(|message| self.apply(message, validation)) {
                Ok(Applied::Changes(changes)) => processed.changes.extend(changes),
                Ok(Applied::Answer(answer)) => processed.outbound.push(*answer),
                Err(error) => processed.outbound.push(RendererMessage::Error(error)),
            }
        }
        processed
    }

    /// The renderer's capabilities: `supportedCatalogIds`, sorted, and with
    /// [`InlineCatalogs::Include`] each catalog's document.
    #[must_use]
    pub fn renderer_capabilities(&self, inline: InlineCatalogs) -> Value {
        let mut capabilities = Map::new();
        capabilities.insert(
            "supportedCatalogIds".to_owned(),
            json!(self.catalogs.ids().collect::<Vec<_>>()),
        );
        if inline == InlineCatalogs::Include {
            let documents: Vec<Value> = self
                .catalogs
                .iter()
                .map(|supported| supported.catalog.to_json())
                .collect();
            capabilities.insert("inlineCatalogs".to_owned(), Value::Array(documents));
        }
        json!({ PROTOCOL_VERSION: capabilities })
    }

    /// The data models of the surfaces created with `sendDataModel`, as a
    /// transport attaches them to a message for the agent, or `None` when
    /// there are none.
    #[must_use]
    pub fn renderer_data_model(&self) -> Option<Value> {
        data_models(
            self.surfaces
                .values()
                .filter(|surface| surface.send_data_model),
        )
    }

    fn apply(
        &mut self,
        message: AgentMessage,
        validation: Validation,
    ) -> Result<Applied, ErrorMessage> {
        match message {
            AgentMessage::CreateSurface(create) => self.create_surface(create, validation),
            AgentMessage::UpdateComponents(update) => {
                let surface = self
                    .surfaces
                    .get(&update.surface_id)
                    .ok_or_else(|| not_found(&update.surface_id))?;
                let staged = self.stage(surface, &update.components, validation)?;
                let surface = self
                    .surfaces
                    .get_mut(&update.surface_id)
                    .ok_or_else(|| not_found(&update.surface_id))?;
                Ok(Applied::Changes(commit(surface, staged)))
            }
            AgentMessage::UpdateDataModel(update) => self.update_data_model(update, validation),
            AgentMessage::DeleteSurface { surface_id } => Ok(Applied::Changes(
                self.surfaces
                    .remove(&surface_id)
                    .map(|_| Change::SurfaceDeleted {
                        surface: surface_id,
                    })
                    .into_iter()
                    .collect(),
            )),
            AgentMessage::CallRendererFunction(call) => {
                Ok(Applied::Answer(Box::new(refuse_call(&call))))
            }
            // The renderer makes no calls to the agent, so there is nothing
            // to answer.
            AgentMessage::AgentFunctionResponse(_) => Ok(Applied::Changes(Vec::new())),
        }
    }

    fn create_surface(
        &mut self,
        create: CreateSurface,
        validation: Validation,
    ) -> Result<Applied, ErrorMessage> {
        let id = create.surface_id;
        if self.surfaces.contains_key(&id) {
            return Err(ErrorMessage::surface(
                ErrorCode::IntegrityError,
                &id,
                format!("Surface '{id}' already exists"),
            ));
        }
        if let Some(catalog) = &create.catalog_id {
            if self.catalogs.get(catalog).is_none() {
                return Err(ErrorMessage::surface(
                    ErrorCode::CatalogNotFound,
                    &id,
                    format!("Catalog '{catalog}' is not supported by this renderer"),
                ));
            }
        }
        let mut surface = Surface {
            id: id.clone(),
            default_catalog: create.catalog_id,
            send_data_model: create.send_data_model,
            metadata: create.metadata,
            data: DataModel::default(),
            components: Vec::new(),
        };
        if let Some(model) = create.data_model {
            if let Validation::Check(_) = validation {
                check_depth(&Value::Object(model.clone()), &id, "/dataModel")?;
            }
            // One write per top-level key, merging into the empty root.
            let mut data = DataModel::default();
            for (key, value) in model {
                let path = Pointer::parse(&format!("/{}", escape(&key)))
                    .map_err(|e| ErrorMessage::surface(ErrorCode::DataError, &id, e.to_string()))?;
                data.set(&path, value).map_err(|e| data_error(&id, &e))?;
            }
            surface.data = data;
        }
        let staged = self.stage(&surface, &create.components, validation)?;
        let mut changes = vec![Change::SurfaceCreated {
            surface: id.clone(),
        }];
        changes.extend(commit(&mut surface, staged));
        self.surfaces.insert(id, surface);
        Ok(Applied::Changes(changes))
    }

    fn update_data_model(
        &mut self,
        update: UpdateDataModel,
        validation: Validation,
    ) -> Result<Applied, ErrorMessage> {
        let id = update.surface_id;
        let surface = self.surfaces.get_mut(&id).ok_or_else(|| not_found(&id))?;
        if let Validation::Check(_) = validation {
            check_depth(&update.value, &id, "/value")?;
        }
        let fired = surface
            .data
            .set(&update.path, update.value)
            .map_err(|e| data_error(&id, &e))?;
        Ok(Applied::Changes(vec![Change::DataUpdated {
            surface: id,
            path: update.path,
            fired,
        }]))
    }

    /// Reads and checks a components list against `surface`, returning the
    /// components it would add or replace. Nothing changes yet.
    fn stage(
        &self,
        surface: &Surface,
        wire: &[WireComponent],
        validation: Validation,
    ) -> Result<Vec<Component>, ErrorMessage> {
        if wire.is_empty() {
            return Ok(Vec::new());
        }
        let mut seen = HashSet::new();
        for (index, entry) in wire.iter().enumerate() {
            if !seen.insert(entry.id.as_str()) {
                return Err(ErrorMessage::at(
                    ErrorCode::IntegrityError,
                    &surface.id,
                    &format!("/components/{index}/id"),
                    format!("Duplicate component ID '{}' in one message", entry.id),
                ));
            }
        }
        let staged = wire
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                self.read_component(surface, entry, validation)
                    .map_err(|error| error.under(&format!("/components/{index}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Validation::Check(config) = validation {
            self.check_graph(surface, &staged, config)?;
        }
        Ok(staged)
    }

    /// Reads one component: resolves its catalog, separates its envelope,
    /// reads its properties by its schema, resolves the catalog of every call
    /// in it, and with validation checks it against those catalogs. Paths in
    /// the error are relative to the component.
    fn read_component(
        &self,
        surface: &Surface,
        entry: &WireComponent,
        validation: Validation,
    ) -> Result<Component, ErrorMessage> {
        let sid = surface.id.as_str();
        let reject = |rejection: Rejection| {
            ErrorMessage::at(
                ErrorCode::ValidationFailed,
                sid,
                &rejection.path,
                rejection.message,
            )
        };
        let config = match validation {
            Validation::Check(config) => {
                for (key, value) in &entry.body {
                    check_depth(value, sid, &format!("/{}", escape(key)))?;
                }
                Some(config)
            }
            Validation::Skip => None,
        };
        let default = surface.default_catalog.as_deref();
        let default_def = default
            .and_then(|id| self.catalogs.get(id))
            .and_then(|supported| supported.catalog.component(&entry.component));
        let mut body = entry.body.clone();
        let named = component::take_catalog_id(&mut body, default_def).map_err(reject)?;
        let (catalog_id, supported) = self
            .catalogs
            .resolve(named.as_deref(), default, sid)
            .map_err(|e| catalog_not_found(sid, &e))?;
        let def = supported.catalog.component(&entry.component);
        let raw = Value::Object(body.clone());
        self.resolve_calls(&raw, default, sid)?;
        let envelope = component::take_envelope(&mut body, def).map_err(reject)?;
        if let Some(config) = config {
            check_schema(entry, &body, def, &envelope, supported, config, sid)?;
            self.check_calls(&raw, default, config, sid)?;
        }
        let properties = component::parse_properties(body, def).map_err(reject)?;
        Ok(Component {
            id: entry.id.clone(),
            kind: entry.component.clone(),
            catalog_id: catalog_id.to_owned(),
            properties,
            accessibility: envelope.accessibility,
            checks: envelope.checks,
            metadata: envelope.metadata,
        })
    }

    /// Resolves the catalog of every call in `value`, which a component holds.
    fn resolve_calls(
        &self,
        value: &Value,
        default: Option<&str>,
        surface: &str,
    ) -> Result<(), ErrorMessage> {
        walk_calls(value, &mut String::new(), 0, &mut |call, path, _| {
            let Some(name) = call.get("@call").and_then(Value::as_str) else {
                return Ok(());
            };
            if name.starts_with('@') {
                return Ok(());
            }
            let named = match call.get("catalogId") {
                None => None,
                Some(Value::String(id)) => Some(id.as_str()),
                Some(_) => {
                    return Err(ErrorMessage::at(
                        ErrorCode::ValidationFailed,
                        surface,
                        &format!("{path}/catalogId"),
                        "a call's catalogId must be a string",
                    ))
                }
            };
            self.catalogs
                .resolve(named, default, surface)
                .map(|_| ())
                .map_err(|e| catalog_not_found(surface, &e))
        })
    }

    /// Checks every call in `value` against the catalog it resolves to: that
    /// it declares the function, lets the renderer call it, and accepts its
    /// arguments; and that calls nest no deeper than [`MAX_CALL_DEPTH`].
    fn check_calls(
        &self,
        value: &Value,
        default: Option<&str>,
        config: ValidationConfig,
        surface: &str,
    ) -> Result<(), ErrorMessage> {
        walk_calls(value, &mut String::new(), 0, &mut |call, path, depth| {
            let invalid = |message: String| {
                ErrorMessage::at(ErrorCode::ValidationFailed, surface, path, message)
            };
            if depth > MAX_CALL_DEPTH {
                return Err(ErrorMessage::at(
                    ErrorCode::RecursionLimitExceeded,
                    surface,
                    path,
                    format!("functionCall depth exceeds the limit of {MAX_CALL_DEPTH}"),
                ));
            }
            let Some(name) = call.get("@call").and_then(Value::as_str) else {
                return Ok(());
            };
            if name.starts_with('@') {
                return if name == "@index" {
                    Ok(())
                } else {
                    Err(invalid(format!(
                        "Unknown system function '{name}': v1.0 defines only @index"
                    )))
                };
            }
            let named = call.get("catalogId").and_then(Value::as_str);
            let (catalog_id, supported) = self
                .catalogs
                .resolve(named, default, surface)
                .map_err(|e| catalog_not_found(surface, &e))?;
            let Some(def) = supported.catalog.function(name) else {
                return if config.allow_unknown_elements {
                    Ok(())
                } else {
                    Err(invalid(format!(
                        "Unrecognized function '{name}' in catalog '{catalog_id}'"
                    )))
                };
            };
            if def.callers() == Callers::AgentOnly {
                return Err(invalid(format!(
                    "Function '{name}' is agentOnly: the renderer cannot call it"
                )));
            }
            let violations = supported
                .validator
                .call_violations(name, &Value::Object(call.clone()))
                .unwrap_or_default();
            first_violation(violations, surface, path)
        })
    }

    /// Composition and topology over the surface as it would be after
    /// `staged`.
    fn check_graph(
        &self,
        surface: &Surface,
        staged: &[Component],
        config: ValidationConfig,
    ) -> Result<(), ErrorMessage> {
        let def_of = |component: &Component| {
            self.catalogs
                .get(&component.catalog_id)
                .and_then(|supported| supported.catalog.component(&component.kind))
        };
        let mut nodes: Vec<Node<'_>> = surface
            .components
            .iter()
            .map(
                |component| match staged.iter().position(|new| new.id == component.id) {
                    Some(index) => Node {
                        component: &staged[index],
                        def: def_of(&staged[index]),
                        index: Some(index),
                    },
                    None => Node {
                        component,
                        def: def_of(component),
                        index: None,
                    },
                },
            )
            .collect();
        for (index, component) in staged.iter().enumerate() {
            if surface.component(&component.id).is_none() {
                nodes.push(Node {
                    component,
                    def: def_of(component),
                    index: Some(index),
                });
            }
        }
        topology::check(&nodes, config, &surface.id)
    }
}

/// The data models of `surfaces` in the shape of `renderer_data_model.json`,
/// or `None` when there are none.
pub(crate) fn data_models<'a>(surfaces: impl Iterator<Item = &'a Surface>) -> Option<Value> {
    let models: Map<String, Value> = surfaces
        .map(|surface| {
            let root = surface
                .data
                .get(&Pointer::root())
                .cloned()
                .unwrap_or(Value::Null);
            (surface.id.clone(), root)
        })
        .collect();
    (!models.is_empty()).then(|| json!({ "version": PROTOCOL_VERSION, "surfaces": models }))
}

/// What applying one message did.
enum Applied {
    Changes(Vec<Change>),
    Answer(Box<RendererMessage>),
}

/// Checks a component against its schema: a type its catalog declares,
/// every property valid and, unless unknown elements are allowed, declared,
/// and `@checks` only on a checkable component.
fn check_schema(
    entry: &WireComponent,
    properties: &Map<String, Value>,
    def: Option<&ComponentDef>,
    envelope: &Envelope,
    supported: &Supported,
    config: ValidationConfig,
    surface: &str,
) -> Result<(), ErrorMessage> {
    let Some(def) = def else {
        return if config.allow_unknown_elements {
            Ok(())
        } else {
            Err(ErrorMessage::at(
                ErrorCode::ValidationFailed,
                surface,
                "/component",
                format!(
                    "Unknown component type '{}' in catalog '{}'",
                    entry.component,
                    supported.catalog.id()
                ),
            ))
        };
    };
    let mut instance = properties.clone();
    instance.insert("id".to_owned(), Value::from(entry.id.as_str()));
    instance.insert(
        "component".to_owned(),
        Value::from(entry.component.as_str()),
    );
    let violations = supported
        .validator
        .schema_violations(&entry.component, &Value::Object(instance))
        .unwrap_or_default();
    first_violation(violations, surface, "")?;
    if !config.allow_unknown_elements {
        let declared = |key: &String| def.properties().contains(key) || def.own.contains(key);
        if let Some(unknown) = properties.keys().find(|key| !declared(key)) {
            return Err(ErrorMessage::at(
                ErrorCode::ValidationFailed,
                surface,
                &format!("/{}", escape(unknown)),
                format!(
                    "Component '{}' ({}) has unknown property '{unknown}'",
                    entry.id, entry.component
                ),
            ));
        }
    }
    if envelope.has_checks && !def.properties().iter().any(|name| name == "@checks") {
        return Err(ErrorMessage::at(
            ErrorCode::ValidationFailed,
            surface,
            "/@checks",
            format!(
                "Component type '{}' is not checkable, so it takes no @checks",
                entry.component
            ),
        ));
    }
    Ok(())
}

/// The first violation, as an error at `prefix` and the violation's path.
fn first_violation(
    violations: Vec<Violation>,
    surface: &str,
    prefix: &str,
) -> Result<(), ErrorMessage> {
    match violations.into_iter().next() {
        None => Ok(()),
        Some(Violation::Schema { path, message }) => Err(ErrorMessage::at(
            ErrorCode::ValidationFailed,
            surface,
            &format!("{prefix}{path}"),
            message,
        )),
        Some(other) => Err(ErrorMessage::at(
            ErrorCode::ValidationFailed,
            surface,
            prefix,
            other.to_string(),
        )),
    }
}

/// Calls `visit` on every function call in `value`, with its JSON Pointer and
/// how deep it nests in calls, outermost first.
fn walk_calls(
    value: &Value,
    path: &mut String,
    depth: usize,
    visit: &mut impl FnMut(&Map<String, Value>, &str, usize) -> Result<(), ErrorMessage>,
) -> Result<(), ErrorMessage> {
    let (children, depth): (Vec<(String, &Value)>, usize) = match value {
        Value::Object(map) if map.get("@call").is_some_and(Value::is_string) => {
            visit(map, path, depth + 1)?;
            (
                map.get("args")
                    .map(|args| ("args".to_owned(), args))
                    .into_iter()
                    .collect(),
                depth + 1,
            )
        }
        Value::Object(map) => (
            map.iter()
                .map(|(key, child)| (escape(key), child))
                .collect(),
            depth,
        ),
        Value::Array(items) => (
            items
                .iter()
                .enumerate()
                .map(|(index, child)| (index.to_string(), child))
                .collect(),
            depth,
        ),
        _ => return Ok(()),
    };
    for (token, child) in children {
        let len = path.len();
        path.push('/');
        path.push_str(&token);
        let walked = walk_calls(child, path, depth, visit);
        path.truncate(len);
        walked?;
    }
    Ok(())
}

/// Fails when `value` nests objects and lists deeper than [`MAX_NESTING`].
fn check_depth(value: &Value, surface: &str, path: &str) -> Result<(), ErrorMessage> {
    fn depth(value: &Value) -> usize {
        match value {
            Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
            Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
            _ => 0,
        }
    }
    if depth(value) > MAX_NESTING {
        Err(ErrorMessage::at(
            ErrorCode::RecursionLimitExceeded,
            surface,
            path,
            format!(
                "Global recursion limit exceeded: the value nests deeper than {MAX_NESTING} levels"
            ),
        ))
    } else {
        Ok(())
    }
}

/// Applies staged components to `surface`.
fn commit(surface: &mut Surface, staged: Vec<Component>) -> Vec<Change> {
    let sid = surface.id.clone();
    let mut changes = Vec::new();
    for component in staged {
        let id = component.id.clone();
        let event = |make: fn(String, String) -> Change| make(sid.clone(), id.clone());
        match surface
            .components
            .iter_mut()
            .find(|old| old.id == component.id)
        {
            Some(old) if old.kind == component.kind && old.catalog_id == component.catalog_id => {
                *old = component;
                changes.push(event(|surface, component| Change::ComponentUpdated {
                    surface,
                    component,
                }));
            }
            Some(old) => {
                *old = component;
                changes.push(event(|surface, component| Change::ComponentRemoved {
                    surface,
                    component,
                }));
                changes.push(event(|surface, component| Change::ComponentAdded {
                    surface,
                    component,
                }));
            }
            None => {
                surface.components.push(component);
                changes.push(event(|surface, component| Change::ComponentAdded {
                    surface,
                    component,
                }));
            }
        }
    }
    changes
}

/// The answer to a `callRendererFunction`: this renderer runs no function on
/// the agent's behalf.
fn refuse_call(call: &CallRendererFunction) -> RendererMessage {
    RendererMessage::FunctionResponse(FunctionResponse {
        function_call_id: call.function_call_id.clone(),
        outcome: Err(FunctionFailure {
            code: ErrorCode::ExecutionError.as_str().to_owned(),
            message: "this renderer runs no function for the agent".to_owned(),
        }),
    })
}

fn not_found(surface: &str) -> ErrorMessage {
    ErrorMessage::surface(
        ErrorCode::IntegrityError,
        surface,
        format!("Surface not found for message: {surface}"),
    )
}

fn catalog_not_found(surface: &str, error: &UnresolvedCatalog) -> ErrorMessage {
    ErrorMessage::surface(ErrorCode::CatalogNotFound, surface, error.to_string())
}

fn data_error(surface: &str, error: &DataError) -> ErrorMessage {
    ErrorMessage::surface(
        ErrorCode::DataError,
        surface,
        format!("Cannot set path: {error}"),
    )
}

#[cfg(test)]
#[path = "processor_tests.rs"]
mod tests;
