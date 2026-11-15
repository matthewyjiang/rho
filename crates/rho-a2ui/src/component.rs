//! A component on a surface: its envelope, kept apart from its properties,
//! and its properties read by what the catalog's schema says they hold.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::action::Action;
use crate::catalog::ComponentDef;
use crate::expr::Expr;
use crate::message::Metadata;
use crate::slot::Slot;

/// A component as a surface holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) catalog_id: String,
    pub(crate) properties: BTreeMap<String, Property>,
    pub(crate) accessibility: Option<Accessibility>,
    pub(crate) checks: Vec<CheckRule>,
    pub(crate) metadata: Option<Metadata>,
}

/// One property of a component, read by its schema.
#[derive(Debug, Clone, PartialEq)]
pub enum Property {
    /// A value, which may hold bindings and calls.
    Value(Expr),
    /// The id of one child component.
    Child(String),
    /// A list of children.
    Children(ChildList),
    /// What the component does when the user interacts with it.
    Action(Action),
    /// A list some of whose items are children or actions.
    List(Vec<Property>),
    /// An object some of whose members are children or actions.
    Object(BTreeMap<String, Property>),
}

/// A `ChildList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildList {
    /// These components, in order.
    Ids(Vec<String>),
    /// One instance of `component_id` for each item of the data model list
    /// at `path`, each in that item's scope.
    Template {
        /// The list, a path relative to the component's scope or absolute.
        path: String,
        /// The component each item instantiates.
        component_id: String,
    },
}

/// The `@accessibility` envelope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Accessibility {
    /// A short label, a `DynamicString`.
    pub label: Option<Expr>,
    /// A longer description, a `DynamicString`.
    pub description: Option<Expr>,
    /// How assistive technology announces updates.
    pub live: Option<Live>,
    /// Whether to hide it from assistive technology, a `DynamicBoolean`.
    pub hidden: Option<Expr>,
}

/// `AccessibilityAttributes.live`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Live {
    /// `off`.
    Off,
    /// `polite`: announced when the user pauses.
    Polite,
    /// `assertive`: announced at once.
    Assertive,
}

/// One rule of `@checks`.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckRule {
    /// A binding or a call that evaluates to a boolean or a
    /// `ValidationResult`.
    pub condition: Expr,
    /// The message shown when the condition fails.
    pub message: Option<String>,
}

impl Component {
    /// The id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The component type, such as `Text`: the wire's `component`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The id of the catalog it belongs to.
    #[must_use]
    pub fn catalog_id(&self) -> &str {
        &self.catalog_id
    }

    /// The catalog properties, by name.
    #[must_use]
    pub fn properties(&self) -> &BTreeMap<String, Property> {
        &self.properties
    }

    /// `@accessibility`.
    #[must_use]
    pub fn accessibility(&self) -> Option<&Accessibility> {
        self.accessibility.as_ref()
    }

    /// `@checks`, in order.
    #[must_use]
    pub fn checks(&self) -> &[CheckRule] {
        &self.checks
    }

    /// `@metadata`.
    #[must_use]
    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }

    /// Every reference to a child component, in property order.
    pub(crate) fn child_edges(&self) -> Vec<ChildEdge> {
        let mut edges = Vec::new();
        for (name, property) in &self.properties {
            collect_edges(property, name, &format!("/{}", escape(name)), &mut edges);
        }
        edges
    }
}

/// A reference from a component to a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildEdge {
    pub(crate) target: String,
    /// Where it is, for people: `slots[1]`.
    pub(crate) field: String,
    /// Where it is, as a JSON Pointer into the component: `/slots/1`.
    pub(crate) pointer: String,
}

fn collect_edges(property: &Property, field: &str, pointer: &str, edges: &mut Vec<ChildEdge>) {
    let mut push = |target: &str, field: String, pointer: String| {
        edges.push(ChildEdge {
            target: target.to_owned(),
            field,
            pointer,
        });
    };
    match property {
        Property::Value(_) | Property::Action(_) => {}
        Property::Child(id) => push(id, field.to_owned(), pointer.to_owned()),
        Property::Children(ChildList::Ids(ids)) => {
            for (index, id) in ids.iter().enumerate() {
                push(
                    id,
                    format!("{field}[{index}]"),
                    format!("{pointer}/{index}"),
                );
            }
        }
        Property::Children(ChildList::Template { component_id, .. }) => push(
            component_id,
            format!("{field}.componentId"),
            format!("{pointer}/componentId"),
        ),
        Property::List(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_edges(
                    item,
                    &format!("{field}[{index}]"),
                    &format!("{pointer}/{index}"),
                    edges,
                );
            }
        }
        Property::Object(members) => {
            for (name, member) in members {
                collect_edges(
                    member,
                    &format!("{field}.{name}"),
                    &format!("{pointer}/{}", escape(name)),
                    edges,
                );
            }
        }
    }
}

/// Why a component's body cannot be read: what is wrong, and where, as a
/// JSON Pointer into the component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejection {
    pub(crate) path: String,
    pub(crate) message: String,
}

impl Rejection {
    fn at(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// The catalog a component names, taken out of its body: `@catalogId`, or
/// during the v1.0 transition a bare `catalogId` that the component's type in
/// the surface's default catalog does not declare as its own.
pub(crate) fn take_catalog_id(
    body: &mut Map<String, Value>,
    default_def: Option<&ComponentDef>,
) -> Result<Option<String>, Rejection> {
    let (key, value) = take_envelope_key(body, "catalogId", default_def)?;
    match value {
        None => Ok(None),
        Some(Value::String(id)) => Ok(Some(id)),
        Some(_) => Err(Rejection::at(
            format!("/{key}"),
            "catalogId must be a string",
        )),
    }
}

/// The envelope keys other than the catalog id.
#[derive(Debug, Default)]
pub(crate) struct Envelope {
    pub(crate) accessibility: Option<Accessibility>,
    pub(crate) checks: Vec<CheckRule>,
    pub(crate) metadata: Option<Metadata>,
    /// Whether the body had `@checks`, which only a checkable component may.
    pub(crate) has_checks: bool,
}

/// Takes `@accessibility`, `@checks` and `@metadata` out of `body`, or their
/// bare forms where `def` does not declare those as its own.
pub(crate) fn take_envelope(
    body: &mut Map<String, Value>,
    def: Option<&ComponentDef>,
) -> Result<Envelope, Rejection> {
    let mut envelope = Envelope::default();
    if let (key, Some(value)) = take_envelope_key(body, "accessibility", def)? {
        envelope.accessibility =
            Some(accessibility(&value).map_err(|e| Rejection::at(format!("/{key}"), e))?);
    }
    if let (key, Some(value)) = take_envelope_key(body, "metadata", def)? {
        envelope.metadata = Some(
            Metadata::from_json(&value)
                .map_err(|e| Rejection::at(format!("/{key}"), e.to_string()))?,
        );
    }
    if let (key, Some(value)) = take_envelope_key(body, "checks", def)? {
        envelope.has_checks = true;
        envelope.checks =
            checks(&value).map_err(|(index, e)| Rejection::at(format!("/{key}/{index}"), e))?;
    }
    if let Some(key) = body.keys().find(|key| key.starts_with('@')) {
        return Err(Rejection::at(
            format!("/{}", escape(key)),
            format!("{key} is not a component envelope key: escape a literal '@' key as '@@'"),
        ));
    }
    Ok(envelope)
}

/// Takes one envelope key: `@name`, or a bare `name` the component does not
/// declare as its own property. Fails when both are there.
fn take_envelope_key(
    body: &mut Map<String, Value>,
    name: &str,
    def: Option<&ComponentDef>,
) -> Result<(String, Option<Value>), Rejection> {
    let sigil = format!("@{name}");
    let declared = def.is_some_and(|def| def.own.contains(name));
    let bare = if declared { None } else { body.remove(name) };
    match (body.remove(&sigil), bare) {
        (Some(_), Some(_)) => Err(Rejection::at(
            format!("/{name}"),
            format!("both {sigil} and {name} are set: send only {sigil}"),
        )),
        (Some(value), None) => non_null(sigil, value),
        (None, Some(value)) => non_null(name.to_owned(), value),
        (None, None) => Ok((sigil, None)),
    }
}

fn non_null(key: String, value: Value) -> Result<(String, Option<Value>), Rejection> {
    if value.is_null() {
        Err(Rejection::at(
            format!("/{key}"),
            format!("{key} must not be null"),
        ))
    } else {
        Ok((key, Some(value)))
    }
}

fn accessibility(value: &Value) -> Result<Accessibility, String> {
    let map = value
        .as_object()
        .ok_or("@accessibility must be an object")?;
    let mut attributes = Accessibility::default();
    for (key, member) in map {
        let dynamic = || Expr::from_json(member).map_err(|e| format!("@accessibility.{key}: {e}"));
        match key.as_str() {
            "label" => attributes.label = Some(dynamic()?),
            "description" => attributes.description = Some(dynamic()?),
            "hidden" => attributes.hidden = Some(dynamic()?),
            "live" => {
                attributes.live = Some(match member.as_str() {
                    Some("off") => Live::Off,
                    Some("polite") => Live::Polite,
                    Some("assertive") => Live::Assertive,
                    _ => {
                        return Err(
                            "@accessibility.live must be off, polite or assertive".to_owned()
                        )
                    }
                });
            }
            other => return Err(format!("@accessibility has unknown key {other:?}")),
        }
    }
    Ok(attributes)
}

fn checks(value: &Value) -> Result<Vec<CheckRule>, (usize, String)> {
    let rules = value
        .as_array()
        .ok_or((0, "@checks must be a list".to_owned()))?;
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| check_rule(rule).map_err(|e| (index, e)))
        .collect()
}

fn check_rule(rule: &Value) -> Result<CheckRule, String> {
    let map = rule.as_object().ok_or("a check rule must be an object")?;
    if let Some(key) = map
        .keys()
        .find(|key| !matches!(key.as_str(), "condition" | "message"))
    {
        return Err(format!("a check rule has unknown key {key:?}"));
    }
    let condition = match map.get("condition").map(Expr::from_json) {
        Some(Ok(condition @ (Expr::Path(_) | Expr::Call(_)))) => condition,
        Some(Err(e)) => return Err(e.to_string()),
        _ => return Err("a check rule's condition must be a binding or a call".to_owned()),
    };
    let message = match map.get("message") {
        None => None,
        Some(Value::String(message)) => Some(message.clone()),
        Some(_) => return Err("a check rule's message must be a string".to_owned()),
    };
    Ok(CheckRule { condition, message })
}

/// Reads the properties left in `body` by what `def` says they hold.
pub(crate) fn parse_properties(
    body: Map<String, Value>,
    def: Option<&ComponentDef>,
) -> Result<BTreeMap<String, Property>, Rejection> {
    body.into_iter()
        .map(|(name, value)| {
            let slot = def
                .and_then(|def| def.slots.get(&name))
                .unwrap_or(&Slot::Value);
            let pointer = format!("/{}", escape(&name));
            let property = parse_property(&value, slot, &pointer)?;
            Ok((name, property))
        })
        .collect()
}

fn parse_property(value: &Value, slot: &Slot, pointer: &str) -> Result<Property, Rejection> {
    let as_value = || {
        Expr::from_json(value)
            .map(Property::Value)
            .map_err(|e| Rejection::at(pointer, e.to_string()))
    };
    match (slot, value) {
        (Slot::Child, Value::String(id)) => Ok(Property::Child(id.clone())),
        (Slot::ChildList, Value::Array(items)) if items.iter().all(Value::is_string) => {
            let ids = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            Ok(Property::Children(ChildList::Ids(ids)))
        }
        (Slot::ChildList, Value::Object(map)) => {
            match (map.get("path"), map.get("componentId"), map.len()) {
                (Some(Value::String(path)), Some(Value::String(component_id)), 2) => {
                    Ok(Property::Children(ChildList::Template {
                        path: path.clone(),
                        component_id: component_id.clone(),
                    }))
                }
                _ => as_value(),
            }
        }
        (Slot::Action, _) => Action::from_json(value)
            .map(Property::Action)
            .map_err(|e| Rejection::at(pointer, e.to_string())),
        (Slot::List(item), Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, member)| parse_property(member, item, &format!("{pointer}/{index}")))
            .collect::<Result<_, _>>()
            .map(Property::List),
        (Slot::Object(members), Value::Object(map))
            if !map.keys().any(|key| key.starts_with('@')) =>
        {
            map.iter()
                .map(|(name, member)| {
                    let slot = members.get(name).unwrap_or(&Slot::Value);
                    let property =
                        parse_property(member, slot, &format!("{pointer}/{}", escape(name)))?;
                    Ok((name.clone(), property))
                })
                .collect::<Result<_, _>>()
                .map(Property::Object)
        }
        _ => as_value(),
    }
}

/// Encodes one JSON Pointer token (RFC 6901).
pub(crate) fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}
