use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::LazyLock;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::ident::is_identifier;
use crate::schema;
use crate::shape;
use crate::slot::{self, Slot};

/// The component type the protocol reserves for the surface container.
const SURFACE: &str = "Surface";

/// The only protocol version this crate loads catalogs for.
const PROTOCOL_VERSION: &str = "1.0";

/// The version a catalog without `protocolVersion` declares, by the spec.
const DEFAULT_PROTOCOL_VERSION: &str = "0.9";

const COMPONENT_UNION: &str = "anyComponent";
const FUNCTION_UNION: &str = "anyFunction";

/// A v1.0 catalog: the components and functions a surface may use, with the
/// JSON Schema of each.
///
/// A catalog is immutable. It keeps its document as written, so
/// [`Catalog::to_json`] gives back what [`Catalog::from_json`] loaded.
#[derive(Debug, Clone)]
pub struct Catalog {
    id: String,
    protocol_version: String,
    instructions: Option<String>,
    components: BTreeMap<String, ComponentDef>,
    functions: BTreeMap<String, FunctionDef>,
    /// The document's `$defs` as written, or `None` when it has none. The
    /// `anyComponent` and `anyFunction` unions in it are recomputed on output.
    defs: Option<Map<String, Value>>,
    /// The document's other top-level keys, such as `$schema` and `title`.
    extra: Map<String, Value>,
}

/// A component type of a [`Catalog`].
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentDef {
    schema: Value,
    properties: Vec<String>,
    required: Vec<String>,
    /// What each property that is not a plain value holds.
    pub(crate) slots: BTreeMap<String, Slot>,
    /// The property names it declares as its own; see
    /// [`shape::own_property_names`].
    pub(crate) own: BTreeSet<String>,
    allowed_parents: Option<Vec<String>>,
    allowed_children: Option<Vec<String>>,
    metadata: Option<Map<String, Value>>,
}

/// A function of a [`Catalog`].
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    schema: Value,
    args: Value,
    return_type: ReturnType,
    callers: Callers,
    requires_user_activation: bool,
}

/// What a catalog function returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnType {
    /// `string`.
    String,
    /// `number`.
    Number,
    /// `boolean`.
    Boolean,
    /// `array`.
    Array,
    /// `object`.
    Object,
    /// `validationResult`: a check's outcome.
    ValidationResult,
    /// `any`.
    Any,
    /// `void`: the function is called for its effect.
    Void,
}

/// Who may call a catalog function (`allowedCallers`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Callers {
    /// `rendererOnly`, the default: the function runs on the renderer, from
    /// the surface's own bindings and actions.
    #[default]
    RendererOnly,
    /// `agentOnly`: only the agent calls it.
    AgentOnly,
    /// `rendererOrAgent`: either may call it.
    RendererOrAgent,
}

/// Whether an entry is a component or a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A component type.
    Component,
    /// A function.
    Function,
}

/// How [`Catalog::derive`] builds a catalog from another one.
#[derive(Debug, Clone, PartialEq)]
pub struct Derivation {
    /// The new catalog's id.
    pub catalog_id: String,
    /// The components to keep or define, or `None` to keep them all.
    pub components: Option<Vec<Entry>>,
    /// The functions to keep or define, or `None` to keep them all.
    pub functions: Option<Vec<Entry>>,
    /// Instructions for the new catalog. The source's describe the source and
    /// are never kept.
    pub instructions: Option<String>,
}

/// One entry of a [`Derivation`].
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// Keep the source's entry of this name.
    Keep(String),
    /// Define an entry, replacing any of the same name.
    Define {
        /// The entry's name.
        name: String,
        /// Its definition, as it would appear in a catalog document.
        schema: Value,
    },
}

/// Why a catalog document is not a valid v1.0 catalog.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CatalogError {
    /// The document is not a JSON object.
    #[error("a catalog document must be a JSON object")]
    NotAnObject,
    /// The document has no `catalogId`.
    #[error("the catalog has no catalogId")]
    MissingId,
    /// A top-level field has the wrong JSON type.
    #[error("catalog field {field:?} has the wrong type")]
    InvalidField {
        /// The field.
        field: String,
    },
    /// The catalog targets a protocol version other than 1.0.
    #[error("protocol version {0:?} is not supported, only {PROTOCOL_VERSION}")]
    UnsupportedVersion(String),
    /// A component or function definition is not a JSON object.
    #[error("{kind} {name:?} is not a JSON object")]
    InvalidEntry {
        /// What the entry is.
        kind: EntryKind,
        /// Its name.
        name: String,
    },
    /// A component name is not a UAX #31 identifier.
    #[error("component name {0:?} is not a UAX #31 identifier")]
    InvalidComponentName(String),
    /// The catalog defines the reserved `Surface` component.
    #[error("the component name {SURFACE:?} is reserved for the surface container")]
    ReservedComponent,
    /// A component property name is not a UAX #31 identifier.
    #[error(
        "component {component:?} has property {property:?}, which is not a UAX #31 identifier"
    )]
    InvalidPropertyName {
        /// The component.
        component: String,
        /// The property.
        property: String,
    },
    /// `allowedParents` or `allowedChildren` is not a list of names.
    #[error("component {component:?} has an invalid {field}")]
    InvalidComposition {
        /// The component.
        component: String,
        /// `allowedParents` or `allowedChildren`.
        field: &'static str,
    },
    /// A component's `metadata` is not an object holding only `extensions`.
    #[error("component {component:?} has invalid metadata")]
    InvalidMetadata {
        /// The component.
        component: String,
    },
    /// A key of a component's `metadata.extensions` is not a UAX #31
    /// identifier.
    #[error(
        "component {component:?} has metadata extension {key:?}, which is not a UAX #31 identifier"
    )]
    InvalidMetadataKey {
        /// The component.
        component: String,
        /// The key.
        key: String,
    },
    /// A function name is not a UAX #31 identifier. This rejects the
    /// `@`-prefixed names the protocol reserves for system functions.
    #[error("function name {0:?} is not a UAX #31 identifier")]
    InvalidFunctionName(String),
    /// A function argument name is not a UAX #31 identifier.
    #[error("function {function:?} has argument {argument:?}, which is not a UAX #31 identifier")]
    InvalidArgumentName {
        /// The function.
        function: String,
        /// The argument.
        argument: String,
    },
    /// A function declares no `returnType`, or one the protocol lacks.
    #[error("function {0:?} has no valid returnType")]
    InvalidReturnType(String),
    /// A function's `allowedCallers` is not one the protocol defines.
    #[error("function {0:?} has an invalid allowedCallers")]
    InvalidCallers(String),
    /// A function's `requiresUserActivation` is not a boolean.
    #[error("function {0:?} has a requiresUserActivation that is not a boolean")]
    InvalidActivation(String),
    /// A function requires user activation but the agent may call it. Only
    /// the renderer has a user to activate it.
    #[error("function {0:?} requires user activation, so only the renderer may call it")]
    ActivationByAgent(String),
    /// A `$ref` points outside the catalog and `common_types.json`. It is
    /// never fetched.
    #[error("unsupported external $ref {reference:?} at {at}")]
    ExternalRef {
        /// The JSON Pointer of the `$ref` in the document.
        at: String,
        /// The reference.
        reference: String,
    },
    /// A `$ref` points at nothing in the catalog or `common_types.json`.
    #[error("$ref {reference:?} at {at} resolves to nothing")]
    UnresolvedRef {
        /// The JSON Pointer of the `$ref` in the document.
        at: String,
        /// The reference.
        reference: String,
    },
    /// A definition is not a valid JSON Schema, found by
    /// [`CatalogValidator::new`](crate::CatalogValidator::new).
    #[error("{kind} {name:?} is not a valid JSON Schema: {message}")]
    InvalidSchema {
        /// What the entry is.
        kind: EntryKind,
        /// Its name.
        name: String,
        /// What the JSON Schema compiler reported.
        message: String,
    },
    /// [`Catalog::derive`] names an entry the source catalog lacks.
    #[error("catalog {catalog:?} has no {kind} {name:?} to derive from")]
    UnknownEntry {
        /// The source catalog's id.
        catalog: String,
        /// What the entry is.
        kind: EntryKind,
        /// Its name.
        name: String,
    },
}

static BASIC: LazyLock<Catalog> = LazyLock::new(|| {
    let document =
        serde_json::from_str(schema::BASIC_CATALOG).expect("the vendored basic catalog is JSON");
    Catalog::from_json(document).expect("the vendored basic catalog is a valid catalog")
});

impl Catalog {
    /// The published v1.0 basic catalog, compiled into the crate.
    #[must_use]
    pub fn basic() -> &'static Self {
        &BASIC
    }

    /// Loads a v1.0 catalog document.
    ///
    /// Every component and function is checked, then those that the
    /// document's `anyComponent` and `anyFunction` unions leave out, when it
    /// has such `oneOf` unions, are dropped. No `$ref` is fetched: one must
    /// point into the document itself or into `common_types.json`. A local
    /// `#/$defs/X` that the document does not define names the common type
    /// `X`.
    ///
    /// # Errors
    ///
    /// Fails when the document breaks a rule of the v1.0 catalog format:
    /// it lacks a `catalogId`, targets another protocol version, names an
    /// entry, property or argument with something other than a UAX #31
    /// identifier, defines `Surface`, has an external or dangling `$ref`, or
    /// declares a function's metadata wrongly. See [`CatalogError`].
    pub fn from_json(document: Value) -> Result<Self, CatalogError> {
        check_refs(&document, &document, &mut String::new())?;
        let Value::Object(mut document) = document else {
            return Err(CatalogError::NotAnObject);
        };
        let id = take_string(&mut document, "catalogId")?.ok_or(CatalogError::MissingId)?;
        let protocol_version = take_string(&mut document, "protocolVersion")?
            .unwrap_or_else(|| DEFAULT_PROTOCOL_VERSION.to_owned());
        if protocol_version != PROTOCOL_VERSION {
            return Err(CatalogError::UnsupportedVersion(protocol_version));
        }
        let instructions = take_string(&mut document, "instructions")?;
        let components_doc = take_object(&mut document, "components")?.unwrap_or_default();
        let functions_doc = take_object(&mut document, "functions")?.unwrap_or_default();
        let defs = take_object(&mut document, "$defs")?;
        let no_defs = Map::new();
        let leaf_defs = defs.as_ref().unwrap_or(&no_defs);

        let mut components = components_doc
            .into_iter()
            .map(|(name, schema)| {
                let def = ComponentDef::parse(&name, schema, leaf_defs)?;
                Ok((name, def))
            })
            .collect::<Result<BTreeMap<_, _>, CatalogError>>()?;
        let mut functions = functions_doc
            .into_iter()
            .map(|(name, schema)| {
                let def = FunctionDef::parse(&name, schema)?;
                Ok((name, def))
            })
            .collect::<Result<BTreeMap<_, _>, CatalogError>>()?;
        if let Some(listed) = union_members(leaf_defs.get(COMPONENT_UNION), EntryKind::Component) {
            components.retain(|name, _| listed.contains(name.as_str()));
        }
        if let Some(listed) = union_members(leaf_defs.get(FUNCTION_UNION), EntryKind::Function) {
            functions.retain(|name, _| listed.contains(name.as_str()));
        }

        Ok(Self {
            id,
            protocol_version,
            instructions,
            components,
            functions,
            defs,
            extra: document,
        })
    }

    /// The catalog document: the definitions as written, external references
    /// kept and no common types added. The `anyComponent` and `anyFunction`
    /// unions, where the document has them, list this catalog's entries.
    #[must_use]
    pub fn to_json(&self) -> Value {
        Value::Object(self.document())
    }

    fn document(&self) -> Map<String, Value> {
        let mut document = self.extra.clone();
        document.insert("catalogId".to_owned(), Value::from(self.id.as_str()));
        document.insert(
            "protocolVersion".to_owned(),
            Value::from(self.protocol_version.as_str()),
        );
        if let Some(instructions) = &self.instructions {
            document.insert(
                "instructions".to_owned(),
                Value::from(instructions.as_str()),
            );
        }
        let components = self
            .components
            .iter()
            .map(|(name, def)| (name.clone(), def.schema.clone()))
            .collect();
        document.insert("components".to_owned(), Value::Object(components));
        if !self.functions.is_empty() {
            let functions = self
                .functions
                .iter()
                .map(|(name, def)| (name.clone(), def.schema.clone()))
                .collect();
            document.insert("functions".to_owned(), Value::Object(functions));
        }
        if let Some(defs) = &self.defs {
            let mut defs = defs.clone();
            for kind in [EntryKind::Component, EntryKind::Function] {
                let key = kind.union_key();
                if let Some(authored) = defs.get(key) {
                    let names = self.entry_names(kind);
                    let union = if union_members(Some(authored), kind).as_ref() == Some(&names) {
                        authored.clone()
                    } else {
                        union(kind, names)
                    };
                    defs.insert(key.to_owned(), union);
                }
            }
            document.insert("$defs".to_owned(), Value::Object(defs));
        }
        document
    }

    /// Builds a catalog with a new id from some of this catalog's entries and
    /// new ones.
    ///
    /// The new catalog keeps this one's protocol version, document layout and
    /// other top-level keys, except `$id`, `title`, `description` and
    /// `instructions`, which describe this catalog.
    ///
    /// # Errors
    ///
    /// Fails with [`CatalogError::UnknownEntry`] when an [`Entry::Keep`] names
    /// an entry this catalog lacks, and as [`Catalog::from_json`] does when
    /// a new entry is invalid.
    pub fn derive(&self, derivation: Derivation) -> Result<Self, CatalogError> {
        let mut document = self.document();
        for key in ["$id", "title", "description", "instructions"] {
            document.remove(key);
        }
        document.insert("catalogId".to_owned(), Value::from(derivation.catalog_id));
        if let Some(instructions) = derivation.instructions {
            document.insert("instructions".to_owned(), Value::from(instructions));
        }
        let components = self.pick(EntryKind::Component, derivation.components)?;
        let functions = self.pick(EntryKind::Function, derivation.functions)?;
        if let Some(Value::Object(defs)) = document.get_mut("$defs") {
            for (kind, entries) in [
                (EntryKind::Component, &components),
                (EntryKind::Function, &functions),
            ] {
                if defs.contains_key(kind.union_key()) {
                    let names = entries.keys().map(String::as_str);
                    defs.insert(kind.union_key().to_owned(), union(kind, names));
                }
            }
        }
        document.insert("components".to_owned(), Value::Object(components));
        if functions.is_empty() {
            document.remove("functions");
        } else {
            document.insert("functions".to_owned(), Value::Object(functions));
        }
        Self::from_json(Value::Object(document))
    }

    /// The definitions a derived catalog gets for one kind of entry.
    fn pick(
        &self,
        kind: EntryKind,
        entries: Option<Vec<Entry>>,
    ) -> Result<Map<String, Value>, CatalogError> {
        let schema_of = |name: &str| match kind {
            EntryKind::Component => self.components.get(name).map(|def| &def.schema),
            EntryKind::Function => self.functions.get(name).map(|def| &def.schema),
        };
        let Some(entries) = entries else {
            return Ok(self
                .entry_names(kind)
                .into_iter()
                .filter_map(|name| Some((name.to_owned(), schema_of(name)?.clone())))
                .collect());
        };
        let mut picked = Map::new();
        for entry in entries {
            match entry {
                Entry::Keep(name) => {
                    let schema = schema_of(&name).ok_or_else(|| CatalogError::UnknownEntry {
                        catalog: self.id.clone(),
                        kind,
                        name: name.clone(),
                    })?;
                    picked.insert(name, schema.clone());
                }
                Entry::Define { name, schema } => {
                    picked.insert(name, schema);
                }
            }
        }
        Ok(picked)
    }

    /// The catalog id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The protocol version the document declares, `1.0`.
    #[must_use]
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    /// The catalog's guidance for a model writing surfaces with it.
    #[must_use]
    pub fn instructions(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// The component types, by name.
    #[must_use]
    pub fn components(&self) -> &BTreeMap<String, ComponentDef> {
        &self.components
    }

    /// The functions, by name.
    #[must_use]
    pub fn functions(&self) -> &BTreeMap<String, FunctionDef> {
        &self.functions
    }

    /// The component type `name`.
    #[must_use]
    pub fn component(&self, name: &str) -> Option<&ComponentDef> {
        self.components.get(name)
    }

    /// The function `name`.
    #[must_use]
    pub fn function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(name)
    }

    /// The document's own `$defs`, the leaf definitions entries refer to as
    /// `#/$defs/<name>`, with the `anyComponent` and `anyFunction` unions.
    pub(crate) fn defs(&self) -> Option<&Map<String, Value>> {
        self.defs.as_ref()
    }

    /// The names of the entries of one kind, in the order the document's
    /// union lists them when it lists exactly them, else sorted.
    pub(crate) fn ordered_names(&self, kind: EntryKind) -> Vec<&str> {
        let names = self.entry_names(kind);
        let authored = self
            .defs
            .as_ref()
            .and_then(|defs| defs.get(kind.union_key()));
        match authored.and_then(|union| union_refs(union, kind)) {
            Some(listed) if listed.iter().copied().collect::<BTreeSet<_>>() == names => listed,
            _ => names.into_iter().collect(),
        }
    }

    fn entry_names(&self, kind: EntryKind) -> BTreeSet<&str> {
        match kind {
            EntryKind::Component => self.components.keys().map(String::as_str).collect(),
            EntryKind::Function => self.functions.keys().map(String::as_str).collect(),
        }
    }
}

impl ComponentDef {
    fn parse(name: &str, schema: Value, defs: &Map<String, Value>) -> Result<Self, CatalogError> {
        if !is_identifier(name) {
            return Err(CatalogError::InvalidComponentName(name.to_owned()));
        }
        if name == SURFACE {
            return Err(CatalogError::ReservedComponent);
        }
        let Value::Object(definition) = &schema else {
            return Err(CatalogError::InvalidEntry {
                kind: EntryKind::Component,
                name: name.to_owned(),
            });
        };
        let shape = shape::component_shape(definition, defs);
        if let Some(property) = shape.own.iter().find(|property| !is_identifier(property)) {
            return Err(CatalogError::InvalidPropertyName {
                component: name.to_owned(),
                property: property.clone(),
            });
        }
        let names = |field: &'static str| -> Result<Option<Vec<String>>, CatalogError> {
            let invalid = || CatalogError::InvalidComposition {
                component: name.to_owned(),
                field,
            };
            definition
                .get(field)
                .map(|value| {
                    value
                        .as_array()
                        .ok_or_else(invalid)?
                        .iter()
                        .map(|name| name.as_str().map(str::to_owned).ok_or_else(invalid))
                        .collect()
                })
                .transpose()
        };
        let allowed_parents = names("allowedParents")?;
        let allowed_children = names("allowedChildren")?;
        let metadata = definition
            .get("metadata")
            .map(|metadata| parse_metadata(name, metadata))
            .transpose()?;
        Ok(Self {
            slots: slot::slots(&shape.properties, defs),
            own: shape::own_property_names(definition, defs),
            properties: shape.properties.keys().cloned().collect(),
            required: shape.required.into_iter().collect(),
            allowed_parents,
            allowed_children,
            metadata,
            schema,
        })
    }

    /// The definition as the catalog document writes it.
    #[must_use]
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    /// The property names, sorted, with those of the common types it mixes
    /// in (such as `Checkable`'s `@checks`) and without the envelope keys
    /// `id` and `component`.
    #[must_use]
    pub fn properties(&self) -> &[String] {
        &self.properties
    }

    /// The required property names, sorted, without the envelope keys.
    #[must_use]
    pub fn required(&self) -> &[String] {
        &self.required
    }

    /// The component types it may be a child of, `Surface` for the top of a
    /// surface, or `None` when any parent may hold it.
    #[must_use]
    pub fn allowed_parents(&self) -> Option<&[String]> {
        self.allowed_parents.as_deref()
    }

    /// The component types it may hold, or `None` when it may hold any.
    #[must_use]
    pub fn allowed_children(&self) -> Option<&[String]> {
        self.allowed_children.as_deref()
    }

    /// Its static `metadata`, an object whose only key is `extensions`.
    #[must_use]
    pub fn metadata(&self) -> Option<&Map<String, Value>> {
        self.metadata.as_ref()
    }
}

fn parse_metadata(component: &str, metadata: &Value) -> Result<Map<String, Value>, CatalogError> {
    let invalid = || CatalogError::InvalidMetadata {
        component: component.to_owned(),
    };
    let metadata = metadata.as_object().ok_or_else(invalid)?;
    for (key, value) in metadata {
        if key != "extensions" {
            return Err(invalid());
        }
        let extensions = value.as_object().ok_or_else(invalid)?;
        if let Some(key) = extensions.keys().find(|key| !is_identifier(key)) {
            return Err(CatalogError::InvalidMetadataKey {
                component: component.to_owned(),
                key: key.clone(),
            });
        }
    }
    Ok(metadata.clone())
}

impl FunctionDef {
    fn parse(name: &str, schema: Value) -> Result<Self, CatalogError> {
        if !is_identifier(name) {
            return Err(CatalogError::InvalidFunctionName(name.to_owned()));
        }
        let Value::Object(definition) = &schema else {
            return Err(CatalogError::InvalidEntry {
                kind: EntryKind::Function,
                name: name.to_owned(),
            });
        };
        // v1.0 declares `returnType` beside the call schema. A definition in
        // the v0.9 shape declares it as the const of a `returnType` property.
        let return_type = definition
            .get("returnType")
            .or_else(|| {
                definition
                    .get("properties")?
                    .get("returnType")?
                    .get("const")
            })
            .and_then(Value::as_str)
            .and_then(ReturnType::parse)
            .ok_or_else(|| CatalogError::InvalidReturnType(name.to_owned()))?;
        let callers = match definition.get("allowedCallers") {
            None => Callers::default(),
            Some(callers) => callers
                .as_str()
                .and_then(Callers::parse)
                .ok_or_else(|| CatalogError::InvalidCallers(name.to_owned()))?,
        };
        let requires_user_activation = match definition.get("requiresUserActivation") {
            None => false,
            Some(activation) => activation
                .as_bool()
                .ok_or_else(|| CatalogError::InvalidActivation(name.to_owned()))?,
        };
        if requires_user_activation && callers != Callers::RendererOnly {
            return Err(CatalogError::ActivationByAgent(name.to_owned()));
        }
        let args = shape::function_args(definition);
        if let Some(argument) = shape::argument_names(&args)
            .into_iter()
            .find(|argument| !is_identifier(argument))
        {
            return Err(CatalogError::InvalidArgumentName {
                function: name.to_owned(),
                argument: argument.to_owned(),
            });
        }
        Ok(Self {
            schema,
            args,
            return_type,
            callers,
            requires_user_activation,
        })
    }

    /// The definition as the catalog document writes it.
    #[must_use]
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    /// The JSON Schema of the call's `args` object, as written. `{}` when the
    /// definition declares none.
    #[must_use]
    pub fn args(&self) -> &Value {
        &self.args
    }

    /// What the function returns.
    #[must_use]
    pub fn return_type(&self) -> ReturnType {
        self.return_type
    }

    /// Who may call it.
    #[must_use]
    pub fn callers(&self) -> Callers {
        self.callers
    }

    /// Whether it runs only in response to a user's gesture, such as a
    /// click. Such a function is always [`Callers::RendererOnly`].
    #[must_use]
    pub fn requires_user_activation(&self) -> bool {
        self.requires_user_activation
    }
}

impl ReturnType {
    const ALL: [Self; 8] = [
        Self::String,
        Self::Number,
        Self::Boolean,
        Self::Array,
        Self::Object,
        Self::ValidationResult,
        Self::Any,
        Self::Void,
    ];

    /// The name a catalog document uses, such as `validationResult`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Array => "array",
            Self::Object => "object",
            Self::ValidationResult => "validationResult",
            Self::Any => "any",
            Self::Void => "void",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

impl fmt::Display for ReturnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Callers {
    const ALL: [Self; 3] = [Self::RendererOnly, Self::AgentOnly, Self::RendererOrAgent];

    /// The name a catalog document uses, such as `rendererOnly`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RendererOnly => "rendererOnly",
            Self::AgentOnly => "agentOnly",
            Self::RendererOrAgent => "rendererOrAgent",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|callers| callers.as_str() == name)
    }
}

impl fmt::Display for Callers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl EntryKind {
    fn union_key(self) -> &'static str {
        match self {
            Self::Component => COMPONENT_UNION,
            Self::Function => FUNCTION_UNION,
        }
    }

    /// The prefix of a reference to an entry of this kind.
    fn ref_prefix(self) -> &'static str {
        match self {
            Self::Component => "#/components/",
            Self::Function => "#/functions/",
        }
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Component => "component",
            Self::Function => "function",
        })
    }
}

/// The union of entries of one kind that a document's `$defs` holds: a
/// `oneOf` of references, or `{"not": {}}` when there are none.
pub(crate) fn union<'a>(kind: EntryKind, names: impl IntoIterator<Item = &'a str>) -> Value {
    let members: Vec<Value> = names
        .into_iter()
        .map(|name| serde_json::json!({ "$ref": format!("{}{name}", kind.ref_prefix()) }))
        .collect();
    match (kind, members.is_empty()) {
        (_, true) => serde_json::json!({ "not": {} }),
        (EntryKind::Component, false) => {
            serde_json::json!({ "oneOf": members, "discriminator": { "propertyName": "component" } })
        }
        (EntryKind::Function, false) => serde_json::json!({ "oneOf": members }),
    }
}

/// The entry names a `oneOf` union lists, in its order, or `None` when it is
/// not a `oneOf` union.
fn union_refs(union: &Value, kind: EntryKind) -> Option<Vec<&str>> {
    let members = union.get("oneOf")?.as_array()?;
    Some(
        members
            .iter()
            .filter_map(|member| {
                member
                    .get("$ref")?
                    .as_str()?
                    .strip_prefix(kind.ref_prefix())
            })
            .collect(),
    )
}

fn union_members(union: Option<&Value>, kind: EntryKind) -> Option<BTreeSet<&str>> {
    Some(union_refs(union?, kind)?.into_iter().collect())
}

fn take_string(
    document: &mut Map<String, Value>,
    field: &str,
) -> Result<Option<String>, CatalogError> {
    match document.remove(field) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(CatalogError::InvalidField {
            field: field.to_owned(),
        }),
    }
}

fn take_object(
    document: &mut Map<String, Value>,
    field: &str,
) -> Result<Option<Map<String, Value>>, CatalogError> {
    match document.remove(field) {
        None => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value)),
        Some(_) => Err(CatalogError::InvalidField {
            field: field.to_owned(),
        }),
    }
}

/// Checks every `$ref` below `node`, which sits at `at` in `document`: each
/// must resolve within the document or within `common_types.json`.
fn check_refs(node: &Value, document: &Value, at: &mut String) -> Result<(), CatalogError> {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref") {
                check_ref(reference, document, at)?;
            }
            for (key, child) in map {
                descend(child, document, at, &escape(key))?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                descend(child, document, at, &index.to_string())?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn descend(
    child: &Value,
    document: &Value,
    at: &mut String,
    token: &str,
) -> Result<(), CatalogError> {
    let len = at.len();
    at.push('/');
    at.push_str(token);
    let checked = check_refs(child, document, at);
    at.truncate(len);
    checked
}

fn check_ref(reference: &str, document: &Value, at: &str) -> Result<(), CatalogError> {
    let (target, fragment) = reference.split_once('#').unwrap_or((reference, ""));
    let target_is_document = target.is_empty() || target == "catalog.json";
    let target = if target_is_document {
        document
    } else if target == "common_types.json" || target.ends_with("/common_types.json") {
        schema::common_types()
    } else {
        return Err(CatalogError::ExternalRef {
            at: format!("{at}/$ref"),
            reference: reference.to_owned(),
        });
    };
    // A local `#/$defs/X` that the catalog does not define names the common
    // type `X`, as validation bundles it.
    let common = || target_is_document && schema::common_types().pointer(fragment).is_some();
    if target.pointer(fragment).is_some() || (fragment.starts_with("/$defs/") && common()) {
        Ok(())
    } else {
        Err(CatalogError::UnresolvedRef {
            at: format!("{at}/$ref"),
            reference: reference.to_owned(),
        })
    }
}

/// Encodes one JSON Pointer token (RFC 6901).
fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
