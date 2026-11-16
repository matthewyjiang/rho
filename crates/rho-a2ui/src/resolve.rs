//! Binding a component to its surface's data: each property evaluated in a
//! scope, ready for a view to render. A failed binding resolves to null and
//! is reported, so one bad binding never blanks the rest of the surface.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::action::Action;
use crate::coerce::{to_bool, to_number, to_string, validity};
use crate::component::{Accessibility, CheckRule, ChildList, Component, Live, Property};
use crate::data_context::{DataContext, UserActivation};
use crate::expr::{Expr, ExpressionError};
use crate::message::{ErrorCode, ErrorMessage, Metadata};
use crate::pointer::{resolve_path, Pointer};
use crate::processor::MessageProcessor;
use crate::surface::Surface;

/// Where in the data model a component is evaluated: the root, or one item
/// of a list a template repeats over.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Scope {
    path: Option<String>,
    index: Option<usize>,
}

impl Scope {
    /// The root of the data model.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    /// Item `index` of a list, at the absolute `path`, which `@index` reads
    /// and relative bindings resolve against.
    #[must_use]
    pub fn item(path: String, index: usize) -> Self {
        Self {
            path: Some(path),
            index: Some(index),
        }
    }

    /// The absolute path relative bindings resolve against.
    #[must_use]
    pub fn path(&self) -> &str {
        self.path.as_deref().unwrap_or("/")
    }

    /// The item's index, outside the root.
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        self.index
    }
}

/// A component with every property evaluated in one scope.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComponent {
    /// The component's id.
    pub id: String,
    /// Its type, such as `Text`: the wire's `component`.
    pub kind: String,
    /// Its catalog.
    pub catalog_id: String,
    /// Its properties, by name.
    pub properties: BTreeMap<String, Resolved>,
    /// `@accessibility`, evaluated.
    pub accessibility: Option<ResolvedAccessibility>,
    /// The outcome of `@checks`, when it has any.
    pub checks: Option<Checks>,
    /// `@metadata`.
    pub metadata: Option<Metadata>,
    /// The bindings that failed, as `error` messages for the agent.
    pub errors: Vec<ErrorMessage>,
}

/// A property, evaluated.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// A value. A binding that failed is null.
    Value(Value),
    /// One child.
    Child(ChildRef),
    /// A list of children: those named, or one per item of a template's list.
    Children(Vec<ChildRef>),
    /// An action, evaluated only when dispatched.
    Action(Action),
    /// A list holding children or actions.
    List(Vec<Resolved>),
    /// An object holding children or actions.
    Object(BTreeMap<String, Resolved>),
}

/// A child to render, and the scope to resolve it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRef {
    /// The child component's id.
    pub id: String,
    /// Its scope: its parent's, or a template item's.
    pub scope: Scope,
}

/// `@accessibility`, evaluated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedAccessibility {
    /// The label.
    pub label: Option<String>,
    /// The description.
    pub description: Option<String>,
    /// How updates are announced.
    pub live: Option<Live>,
    /// Whether assistive technology skips it.
    pub hidden: bool,
}

/// The outcome of a component's `@checks`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Checks {
    failures: Vec<CheckFailure>,
}

/// One check that failed.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckFailure {
    /// A machine-readable code, such as `INVALID_EMAIL`.
    pub code: Option<String>,
    /// What to show.
    pub message: Option<String>,
    /// How much it matters.
    pub severity: Severity,
    /// Any other members of the `ValidationResult`.
    pub details: Map<String, Value>,
}

/// `ValidationResult.severity`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Blocks: the component is invalid.
    #[default]
    Error,
    /// Shown, but the component stays valid.
    Warning,
    /// Shown, but the component stays valid.
    Info,
}

impl Resolved {
    /// The value, for a property that holds one.
    #[must_use]
    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Value(value) => Some(value),
            _ => None,
        }
    }

    /// The value read as a `DynamicString`: null is empty, other values are
    /// written as text.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        self.value().map(to_string)
    }

    /// The value read as a `DynamicNumber`.
    #[must_use]
    pub fn number(&self) -> Option<f64> {
        self.value().map(to_number)
    }

    /// The value read as a `DynamicBoolean`.
    #[must_use]
    pub fn boolean(&self) -> Option<bool> {
        self.value().map(to_bool)
    }
}

impl Checks {
    /// Whether no check failed with severity [`Severity::Error`].
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self
            .failures
            .iter()
            .any(|failure| failure.severity == Severity::Error)
    }

    /// The messages of the failed checks of severity error, in order.
    #[must_use]
    pub fn errors(&self) -> Vec<&str> {
        self.failures
            .iter()
            .filter(|failure| failure.severity == Severity::Error)
            .filter_map(|failure| failure.message.as_deref())
            .collect()
    }

    /// Every failed check, in order, whatever its severity.
    #[must_use]
    pub fn failures(&self) -> &[CheckFailure] {
        &self.failures
    }
}

impl CheckFailure {
    /// The failure as a `ValidationResult`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut result = self.details.clone();
        result.insert("valid".to_owned(), Value::Bool(false));
        if let Some(code) = &self.code {
            result.insert("code".to_owned(), Value::from(code.as_str()));
        }
        if let Some(message) = &self.message {
            result.insert("message".to_owned(), Value::from(message.as_str()));
        }
        result.insert("severity".to_owned(), Value::from(self.severity.as_str()));
        Value::Object(result)
    }

    /// Reads a failed `ValidationResult`, its severity `error` by default.
    fn from_result(mut result: Map<String, Value>) -> Self {
        result.remove("valid");
        let text = |result: &mut Map<String, Value>, key: &str| match result.remove(key) {
            Some(Value::String(text)) => Some(text),
            _ => None,
        };
        let code = text(&mut result, "code");
        let message = text(&mut result, "message");
        let severity = match text(&mut result, "severity").as_deref() {
            Some("warning") => Severity::Warning,
            Some("info") => Severity::Info,
            _ => Severity::Error,
        };
        Self {
            code,
            message,
            severity,
            details: result,
        }
    }
}

impl Severity {
    /// The name a `ValidationResult` uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

impl MessageProcessor {
    /// Evaluates `expr` on surface `surface`, in `scope`, as a binding does,
    /// or `None` when there is no such surface.
    #[must_use]
    pub fn evaluate(
        &self,
        surface: &str,
        expr: &Expr,
        scope: &Scope,
    ) -> Option<Result<Value, ExpressionError>> {
        let surface = self.surfaces.get(surface)?;
        Some(
            self.context(surface, scope, UserActivation::Absent)
                .evaluate(expr),
        )
    }

    /// Resolves component `component` of surface `surface` in `scope`, or
    /// `None` when there is no such component.
    #[must_use]
    pub fn resolve(
        &self,
        surface: &str,
        component: &str,
        scope: &Scope,
    ) -> Option<ResolvedComponent> {
        let surface = self.surfaces.get(surface)?;
        let component = surface.component(component)?;
        let mut binder = Binder {
            processor: self,
            surface,
            component,
            scope,
            errors: Vec::new(),
        };
        let properties = component
            .properties
            .iter()
            .map(|(name, property)| (name.clone(), binder.property(name, property)))
            .collect();
        let accessibility = component
            .accessibility
            .as_ref()
            .map(|a| binder.accessibility(a));
        let checks = (!component.checks.is_empty()).then(|| binder.checks(&component.checks));
        Some(ResolvedComponent {
            id: component.id.clone(),
            kind: component.kind.clone(),
            catalog_id: component.catalog_id.clone(),
            properties,
            accessibility,
            checks,
            metadata: component.metadata.clone(),
            errors: binder.errors,
        })
    }

    /// A context on `surface`'s data model, in `scope`.
    pub(crate) fn context<'a>(
        &'a self,
        surface: &'a Surface,
        scope: &Scope,
        activation: UserActivation,
    ) -> DataContext<'a> {
        let mut context = DataContext::on_surface(
            &surface.data,
            &self.catalogs,
            &surface.id,
            surface.default_catalog.as_deref(),
        )
        .with_activation(activation);
        if let Some(path) = &scope.path {
            context = context.with_scope(path);
        }
        if let Some(index) = scope.index {
            context = context.with_index(index);
        }
        context
    }
}

/// The `error` an evaluation failure is reported as: `CATALOG_ERROR` when the
/// call's catalog cannot be resolved, else `EXPRESSION_ERROR`.
pub(crate) fn evaluation_error(surface: &str, what: &str, error: &ExpressionError) -> ErrorMessage {
    let code = match error {
        ExpressionError::UnresolvedCatalog(_) => ErrorCode::CatalogError,
        _ => ErrorCode::ExpressionError,
    };
    ErrorMessage::surface(code, surface, format!("{what}: {error}"))
}

struct Binder<'a> {
    processor: &'a MessageProcessor,
    surface: &'a Surface,
    component: &'a Component,
    scope: &'a Scope,
    errors: Vec<ErrorMessage>,
}

impl Binder<'_> {
    fn evaluate(&mut self, what: &str, expr: &Expr) -> Result<Value, ExpressionError> {
        let outcome = self
            .processor
            .context(self.surface, self.scope, UserActivation::Absent)
            .evaluate(expr);
        if let Err(error) = &outcome {
            let what = format!("component '{}' {what}", self.component.id);
            self.errors
                .push(evaluation_error(&self.surface.id, &what, error));
        }
        outcome
    }

    fn property(&mut self, name: &str, property: &Property) -> Resolved {
        match property {
            Property::Value(expr) => Resolved::Value(
                self.evaluate(&format!("property '{name}'"), expr)
                    .unwrap_or(Value::Null),
            ),
            Property::Child(id) => Resolved::Child(ChildRef {
                id: id.clone(),
                scope: self.scope.clone(),
            }),
            Property::Children(ChildList::Ids(ids)) => Resolved::Children(
                ids.iter()
                    .map(|id| ChildRef {
                        id: id.clone(),
                        scope: self.scope.clone(),
                    })
                    .collect(),
            ),
            Property::Children(ChildList::Template { path, component_id }) => {
                Resolved::Children(self.template(path, component_id))
            }
            Property::Action(action) => Resolved::Action(action.clone()),
            Property::List(items) => {
                Resolved::List(items.iter().map(|item| self.property(name, item)).collect())
            }
            Property::Object(members) => Resolved::Object(
                members
                    .iter()
                    .map(|(key, member)| (key.clone(), self.property(name, member)))
                    .collect(),
            ),
        }
    }

    /// One instance of `component_id` per item of the list at `path`.
    fn template(&self, path: &str, component_id: &str) -> Vec<ChildRef> {
        let list = resolve_path(path, self.scope.path.as_deref());
        let items = Pointer::parse(&list)
            .ok()
            .and_then(|pointer| self.surface.data.get(&pointer))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let base = list.trim_end_matches('/');
        (0..items)
            .map(|index| ChildRef {
                id: component_id.to_owned(),
                scope: Scope::item(format!("{base}/{index}"), index),
            })
            .collect()
    }

    fn accessibility(&mut self, attributes: &Accessibility) -> ResolvedAccessibility {
        let text = |binder: &mut Self, what: &str, expr: &Option<Expr>| {
            expr.as_ref()
                .map(|expr| to_string(&binder.evaluate(what, expr).unwrap_or(Value::Null)))
        };
        ResolvedAccessibility {
            label: text(self, "@accessibility.label", &attributes.label),
            description: text(self, "@accessibility.description", &attributes.description),
            live: attributes.live,
            hidden: attributes.hidden.as_ref().is_some_and(|hidden| {
                to_bool(
                    &self
                        .evaluate("@accessibility.hidden", hidden)
                        .unwrap_or(Value::Null),
                )
            }),
        }
    }

    /// Evaluates each rule: a boolean or a `ValidationResult`, a failure
    /// taking the rule's message for an evaluated call, and as a fallback for
    /// a bound result, which carries its own.
    fn checks(&mut self, rules: &[CheckRule]) -> Checks {
        let mut failures = Vec::new();
        for (index, rule) in rules.iter().enumerate() {
            let outcome = self.evaluate(&format!("@checks[{index}]"), &rule.condition);
            let mut result = match outcome {
                Ok(Value::Object(result)) if validity(&Value::Object(result.clone())).is_some() => {
                    result
                }
                Ok(value) => {
                    let mut result = Map::new();
                    result.insert("valid".to_owned(), Value::Bool(to_bool(&value)));
                    result
                }
                Err(error) => {
                    let mut result = Map::new();
                    result.insert("valid".to_owned(), Value::Bool(false));
                    let message = rule.message.clone().unwrap_or_else(|| error.to_string());
                    result.insert("message".to_owned(), Value::from(message));
                    result
                }
            };
            if result.get("valid") == Some(&Value::Bool(true)) {
                continue;
            }
            let bound = matches!(rule.condition, Expr::Path(_));
            if let Some(message) = &rule.message {
                if !bound || !result.get("message").is_some_and(Value::is_string) {
                    result.insert("message".to_owned(), Value::from(message.as_str()));
                }
            }
            failures.push(CheckFailure::from_result(result));
        }
        Checks { failures }
    }
}
