//! Validation of component instances and function calls against a catalog,
//! and of values against the common types. No reference is ever fetched:
//! the only documents a validator can reach are those compiled into the
//! crate.

use std::collections::HashMap;
use std::fmt::{self, Write as _};

use jsonschema::error::ValidationErrorKind;
use jsonschema::{Retrieve, Uri};
use serde_json::{json, Map, Value};
use thiserror::Error;

use crate::catalog::{Catalog, CatalogError, EntryKind};
use crate::schema;

/// Where `common_types.json` resolves its `catalog.json` references to.
const CATALOG_URI: &str = "https://a2ui.org/specification/v1_0/catalog.json";

/// A UAX #31 identifier, as a JSON Schema pattern.
const IDENTIFIER_PATTERN: &str = r"^[\p{XID_Start}_][\p{XID_Continue}]*$";

/// Checks component instances and function calls against one catalog, with
/// its schemas compiled once.
///
/// A function call runs in the catalog its `catalogId` names, else in the
/// surface's default catalog. The validator fully checks the calls that run
/// in its catalog: those naming it, and those naming none in a component
/// without `@catalogId`, which it takes to be in the surface default. Of any
/// other call it checks the envelope only, with UAX #31 function and
/// argument names, since another catalog defines it.
#[derive(Clone)]
pub struct CatalogValidator {
    catalog_id: String,
    components: HashMap<String, jsonschema::Validator>,
    functions: HashMap<String, jsonschema::Validator>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CatalogValidator>();
};

/// Why a value failed validation: every violation found.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub struct ValidationError {
    violations: Vec<Violation>,
}

/// The catalog a component instance belongs to, as far as the validator can
/// tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    /// It names no `@catalogId`, so it is in the surface's default catalog.
    SurfaceDefault,
    /// It names a `@catalogId`, which may not be the surface's default.
    Named,
}

/// One way a value breaks its schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// The value is not an object with a string `component`.
    NotAComponent,
    /// `component` names no component of the catalog.
    UnknownComponent(String),
    /// A call that runs in the catalog names a function it lacks.
    UnknownFunction {
        /// The JSON Pointer of the call in the validated value.
        path: String,
        /// The function name.
        name: String,
    },
    /// [`validate_common_type`] names a type `common_types.json` lacks.
    UnknownType(String),
    /// The value, or a part of it, does not match its JSON Schema.
    Schema {
        /// The JSON Pointer of the offending part in the validated value.
        path: String,
        /// What is wrong.
        message: String,
    },
}

impl CatalogValidator {
    /// Compiles the schemas of `catalog`'s components and functions.
    ///
    /// # Errors
    ///
    /// Fails with [`CatalogError::InvalidSchema`] when a definition is not a
    /// valid JSON Schema, such as one with a malformed `pattern`.
    pub fn new(catalog: &Catalog) -> Result<Self, CatalogError> {
        let mut bundle = catalog.validation_document();
        let defs = bundle
            .entry("$defs")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(defs) = defs {
            // A call that names another catalog is checked there, so the
            // component schemas check only its envelope.
            defs.insert("anyFunction".to_owned(), call_envelope());
            if let Some(common) = schema::common_type("FunctionCommon") {
                defs.entry("FunctionCommon")
                    .or_insert_with(|| common.clone());
            }
        }
        let build = |kind: EntryKind, name: &str, root: Value| {
            let mut document = bundle.clone();
            if let Value::Object(root) = root {
                document.extend(root);
            }
            compile(&Value::Object(document)).map_err(|message| CatalogError::InvalidSchema {
                kind,
                name: name.to_owned(),
                message,
            })
        };
        let components = catalog
            .components()
            .keys()
            .map(|name| {
                let root = json!({ "$ref": format!("#/components/{}", fragment_token(name)) });
                Ok((name.clone(), build(EntryKind::Component, name, root)?))
            })
            .collect::<Result<_, CatalogError>>()?;
        let functions = catalog
            .functions()
            .keys()
            .map(|name| {
                let root = json!({
                    "allOf": [
                        { "$ref": "#/$defs/FunctionCommon" },
                        { "$ref": format!("#/functions/{}", fragment_token(name)) },
                    ],
                    "unevaluatedProperties": false,
                });
                Ok((name.clone(), build(EntryKind::Function, name, root)?))
            })
            .collect::<Result<_, CatalogError>>()?;
        Ok(Self {
            catalog_id: catalog.id().to_owned(),
            components,
            functions,
        })
    }

    /// Validates one component instance, such as an entry of an
    /// `updateComponents` message, and the function calls in it.
    ///
    /// `@catalogId` is part of the envelope, not of the component's schema.
    ///
    /// # Errors
    ///
    /// Fails with every [`Violation`] found.
    pub fn validate_component(&self, instance: &Value) -> Result<(), ValidationError> {
        let Some(name) = instance.get("component").and_then(Value::as_str) else {
            return Err(Violation::NotAComponent.into());
        };
        let Some(validator) = self.components.get(name) else {
            return Err(Violation::UnknownComponent(name.to_owned()).into());
        };
        let mut violations = Vec::new();
        let mut body = instance.clone();
        let home = match body
            .as_object_mut()
            .and_then(|map| map.remove("@catalogId"))
        {
            None => Home::SurfaceDefault,
            Some(Value::String(_)) => Home::Named,
            Some(_) => {
                violations.push(Violation::Schema {
                    path: "/@catalogId".to_owned(),
                    message: "@catalogId is not a string".to_owned(),
                });
                Home::Named
            }
        };
        schema_violations(validator, &body, "", &mut violations);
        self.check_calls(&body, home, &mut String::new(), &mut violations);
        ValidationError::check(violations)
    }

    /// The violations of a component's own schema, with every call in it
    /// checked as an envelope only, or `None` when the catalog has no such
    /// component. The processor checks each call in the catalog it resolves
    /// to with [`CatalogValidator::call_violations`].
    pub(crate) fn schema_violations(
        &self,
        component: &str,
        instance: &Value,
    ) -> Option<Vec<Violation>> {
        let validator = self.components.get(component)?;
        let mut violations = Vec::new();
        schema_violations(validator, instance, "", &mut violations);
        Some(violations)
    }

    /// The violations of one call to a function of this catalog, or `None`
    /// when the catalog has no such function.
    pub(crate) fn call_violations(&self, name: &str, call: &Value) -> Option<Vec<Violation>> {
        let validator = self.functions.get(name)?;
        let mut violations = Vec::new();
        schema_violations(validator, call, "", &mut violations);
        Some(violations)
    }

    /// Fully checks every call below `node` that runs in this catalog.
    fn check_calls(
        &self,
        node: &Value,
        home: Home,
        path: &mut String,
        violations: &mut Vec<Violation>,
    ) {
        match node {
            Value::Object(map) => {
                if let Some(Value::String(name)) = map.get("@call") {
                    self.check_call(node, name, home, path, violations);
                }
                for (key, child) in map {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&key.replace('~', "~0").replace('/', "~1"));
                    self.check_calls(child, home, path, violations);
                    path.truncate(len);
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    let len = path.len();
                    path.push('/');
                    path.push_str(&index.to_string());
                    self.check_calls(child, home, path, violations);
                    path.truncate(len);
                }
            }
            _ => {}
        }
    }

    fn check_call(
        &self,
        call: &Value,
        name: &str,
        home: Home,
        path: &str,
        violations: &mut Vec<Violation>,
    ) {
        // A system function such as `@index` belongs to no catalog; the
        // envelope schema checks it.
        if name.starts_with('@') {
            return;
        }
        let runs_here = match call.get("catalogId") {
            None => home == Home::SurfaceDefault,
            Some(Value::String(id)) => *id == self.catalog_id,
            // The envelope schema rejects a catalogId that is not a string.
            Some(_) => false,
        };
        if !runs_here {
            return;
        }
        match self.functions.get(name) {
            Some(validator) => schema_violations(validator, call, path, violations),
            None => violations.push(Violation::UnknownFunction {
                path: path.to_owned(),
                name: name.to_owned(),
            }),
        }
    }
}

impl fmt::Debug for CatalogValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut components: Vec<&String> = self.components.keys().collect();
        let mut functions: Vec<&String> = self.functions.keys().collect();
        components.sort();
        functions.sort();
        f.debug_struct("CatalogValidator")
            .field("catalog_id", &self.catalog_id)
            .field("components", &components)
            .field("functions", &functions)
            .finish()
    }
}

/// Validates `value` as the common type `name` of `common_types.json`.
///
/// A function call in it is checked as an envelope, since no catalog is at
/// hand: a UAX #31 function name with arguments under UAX #31 names, or a
/// call to `@index`.
///
/// # Errors
///
/// Fails with every [`Violation`] found, or with
/// [`Violation::UnknownType`] when `common_types.json` has no type `name`.
///
/// # Panics
///
/// Panics if a vendored common type is not a valid JSON Schema, a bug in
/// this crate.
pub fn validate_common_type(name: &str, value: &Value) -> Result<(), ValidationError> {
    if schema::common_type(name).is_none() {
        return Err(Violation::UnknownType(name.to_owned()).into());
    }
    let mut violations = Vec::new();
    schema_violations(&common_type_validator(name), value, "", &mut violations);
    ValidationError::check(violations)
}

impl ValidationError {
    fn check(violations: Vec<Violation>) -> Result<(), Self> {
        if violations.is_empty() {
            Ok(())
        } else {
            Err(Self { violations })
        }
    }

    /// The violations, at least one.
    #[must_use]
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }
}

impl From<Violation> for ValidationError {
    fn from(violation: Violation) -> Self {
        Self {
            violations: vec![violation],
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, violation) in self.violations.iter().enumerate() {
            if n > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{violation}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAComponent => f.write_str("not an object with a string component"),
            Self::UnknownComponent(name) => write!(f, "unknown component {name:?}"),
            Self::UnknownFunction { path, name } => write!(f, "{path}: unknown function {name:?}"),
            Self::UnknownType(name) => write!(f, "no common type {name:?}"),
            Self::Schema { path, message } => write!(f, "{path}: {message}"),
        }
    }
}

/// The schema of a call to a function of a catalog the validator does not
/// hold: a function name and arguments under UAX #31 names. `@index` is not
/// one, so that a call to it matches only `IndexSystemFunction`.
fn call_envelope() -> Value {
    json!({
        "type": "object",
        "properties": {
            "@call": { "type": "string", "pattern": IDENTIFIER_PATTERN },
            "args": { "type": "object", "propertyNames": { "pattern": IDENTIFIER_PATTERN } },
        },
        "required": ["@call"],
    })
}

/// `name` as a token of a JSON Pointer in a URI fragment: escaped as RFC 6901
/// says, then percent-encoded where RFC 3986 does not allow it in a fragment,
/// as a non-ASCII identifier is.
fn fragment_token(name: &str) -> String {
    let escaped = name.replace('~', "~0").replace('/', "~1");
    let mut token = String::with_capacity(escaped.len());
    for byte in escaped.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            token.push(char::from(byte));
        } else {
            let _ = write!(token, "%{byte:02X}");
        }
    }
    token
}

fn common_type_validator(name: &str) -> jsonschema::Validator {
    let root = json!({ "$ref": format!("{}#/$defs/{name}", schema::COMMON_TYPES_URI) });
    compile(&root)
        .unwrap_or_else(|e| panic!("the vendored common type {name} does not compile: {e}"))
}

fn compile(schema: &Value) -> Result<jsonschema::Validator, String> {
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .with_retriever(Embedded)
        .build(schema)
        .map_err(|e| e.to_string())
}

fn schema_violations(
    validator: &jsonschema::Validator,
    value: &Value,
    path: &str,
    violations: &mut Vec<Violation>,
) {
    violations.extend(validator.iter_errors(value).map(|error| {
        // A missing or unexpected property is reported at that property, not
        // at the object that lacks or holds it.
        let field = match error.kind() {
            ValidationErrorKind::Required { property } => property.as_str().map(str::to_owned),
            ValidationErrorKind::AdditionalProperties { unexpected }
            | ValidationErrorKind::UnevaluatedProperties { unexpected } => {
                unexpected.first().cloned()
            }
            _ => None,
        };
        let mut at = format!("{path}{}", error.instance_path().as_str());
        if let Some(field) = field {
            at.push('/');
            at.push_str(&field.replace('~', "~0").replace('/', "~1"));
        }
        Violation::Schema {
            path: at,
            message: error.to_string(),
        }
    }));
}

/// Resolves the documents compiled into the crate, and nothing else.
struct Embedded;

impl Retrieve for Embedded {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        match uri.as_str() {
            schema::COMMON_TYPES_URI => Ok(schema::common_types().clone()),
            CATALOG_URI => Ok(json!({ "$defs": { "anyFunction": call_envelope() } })),
            other => Err(format!("{other} is not a document compiled into rho-a2ui").into()),
        }
    }
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
