//! Compiling a component call: mapping its arguments to properties, and its
//! checks.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use super::{component_id, Compiler, Kind, Slot};
use crate::express::error::{ExpressError, Position};
use crate::express::parser::{Call, Check, Node, NodeKind};
use crate::express::signature::{self, ComponentSignature, Signatures, SlotKind};

impl<'a> Compiler<'_, 'a> {
    /// Compiles a call to a component under the variable or synthesized
    /// name `name`. Inline components it holds go to `state.extras`.
    pub(super) fn component(&mut self, name: &str, call: &'a Call) -> Result<Value, ExpressError> {
        let express = self.express;
        let (catalog, write_catalog) =
            express.resolve(Kind::Component, &call.name, call.kwarg("catalogId"))?;
        let signatures = self.signatures(catalog);
        let sig = &signatures.components[&call.name];
        let id = component_id(name, call)?;
        let mut component = Map::new();
        component.insert("id".to_owned(), Value::String(id.clone()));
        component.insert("component".to_owned(), Value::String(call.name.clone()));
        if let Some(catalog_id) = write_catalog {
            component.insert("@catalogId".to_owned(), Value::String(catalog_id));
        }
        let declares = |param: &str| sig.params.iter().any(|p| p == param);
        let extensions_kw = (!declares("extensions"))
            .then(|| call.kwarg("extensions"))
            .flatten();
        if let Some(node) = extensions_kw {
            let extensions = self.extensions(node, &format!("component {}", call.name))?;
            component.insert("@metadata".to_owned(), json!({ "extensions": extensions }));
        }
        let reserved = |key: &str| {
            matches!(key, "catalogId" | "id") || (key == "extensions" && extensions_kw.is_some())
        };
        let kwargs: Vec<(&str, &'a Node)> = call
            .kwargs
            .iter()
            .filter(|(key, _)| !reserved(key))
            .map(|(key, node)| (key.as_str(), node))
            .collect();

        let mut checks: Vec<(&'a Check, Position)> = Vec::new();
        let positional: Vec<&'a Node> = call
            .args
            .iter()
            .filter(|node| !collect_checks(node, &mut checks))
            .collect();
        let mut pairs = positional_pairs(signatures, sig, &call.name, &id, &positional, &kwargs)?;
        for &(key, node) in &kwargs {
            if !collect_checks(node, &mut checks) {
                pairs.push((key.to_owned(), node));
            }
        }

        let mut seen = HashSet::new();
        let mut skipped = HashSet::new();
        let mut value_path = None;
        for (prop, node) in pairs {
            if !declares(&prop) {
                return Err(unknown_property(&call.name, &prop, sig).at(node.pos));
            }
            if !seen.insert(prop.clone()) {
                return Err(ExpressError::validation(format!(
                    "property {prop} of component {} is given twice",
                    call.name
                ))
                .at(node.pos));
            }
            if matches!(node.kind, NodeKind::Skipped) {
                skipped.insert(prop);
                continue;
            }
            let value = self
                .property(catalog, sig, &call.name, &id, &prop, node)
                .map_err(|e| e.at(node.pos))?;
            if prop == "value" && self.resolves_to_path(node) {
                value_path = Some(value.clone());
            }
            if !value.is_null() {
                component.insert(prop, value);
            }
        }

        if let Some(&(_, first)) = checks.first() {
            if !sig.checkable && !declares("checks") {
                return Err(ExpressError::validation(format!(
                    "component {} takes no checks: its catalog schema declares no checks property",
                    call.name
                ))
                .at(first));
            }
            let compiled = self.checks(&checks, value_path)?;
            let key = if sig.takes_envelope_checks() {
                "@checks"
            } else {
                "checks"
            };
            component.insert(key.to_owned(), compiled);
        }

        check_required(sig, &call.name, &component, &skipped)?;
        Ok(Value::Object(component))
    }

    /// Compiles a component's checks, each passed the component's bound
    /// value, `value_path`.
    fn checks(
        &mut self,
        checks: &[(&'a Check, Position)],
        value_path: Option<Value>,
    ) -> Result<Value, ExpressError> {
        self.state.value_path = value_path;
        let compiled = checks
            .iter()
            .map(|&(check, pos)| {
                let (condition, message) = self.check(check).map_err(|e| e.at(pos))?;
                Ok(json!({ "condition": condition, "message": message }))
            })
            .collect::<Result<Vec<_>, ExpressError>>();
        self.state.value_path = None;
        compiled.map(Value::Array)
    }

    /// Compiles the value of one component property.
    fn property(
        &mut self,
        catalog: usize,
        sig: &ComponentSignature,
        component: &str,
        id: &str,
        prop: &str,
        node: &'a Node,
    ) -> Result<Value, ExpressError> {
        let schema = sig.schema(prop);
        let kind = schema.and_then(signature::slot_kind);
        let slot = Slot {
            action: schema.is_some_and(signature::is_action),
            parent: Some((id, prop)),
            index: None,
            component_ref: kind == Some(SlotKind::Child),
            list_refs: kind == Some(SlotKind::ChildList) && matches!(node.kind, NodeKind::Array(_)),
            schema: schema.map(|s| (s.clone(), catalog)),
        };
        let mut value = self.value(node, &slot)?;
        if let Some(schema) = schema {
            if !signature::allows_binding(schema) {
                if self.has_binding(node, &mut Vec::new()) {
                    return Err(ExpressError::validation(format!(
                        "property {prop} of component {component} takes no data binding: it must be a literal value"
                    )));
                }
                if signature::expects_options(schema) {
                    if let Value::Array(items) = &mut value {
                        for item in items.iter_mut() {
                            if let Value::String(text) = item {
                                *item = json!({ "label": text, "value": text });
                            }
                        }
                    }
                }
            }
            if let Some(values) = signature::find_enum(schema) {
                if !value.is_object() && !value.is_array() && !values.contains(&value) {
                    let allowed: Vec<String> = values.iter().map(Value::to_string).collect();
                    return Err(ExpressError::validation(format!(
                        "property {prop} of component {component} does not take {value}; it takes {}",
                        allowed.join(", ")
                    )));
                }
            }
        }
        Ok(value)
    }

    /// Whether `node` writes a data binding outside any call, following
    /// variables. Map literals are searched whatever their keys.
    fn has_binding(&self, node: &Node, seen: &mut Vec<String>) -> bool {
        match &node.kind {
            NodeKind::Path(_) => true,
            NodeKind::Variable(name) => {
                if seen.contains(name) {
                    return false;
                }
                let Some(value) = self.scope.symbols.get(name) else {
                    return false;
                };
                seen.push(name.clone());
                let found = self.has_binding(value, seen);
                seen.pop();
                found
            }
            NodeKind::Map(entries) => entries
                .iter()
                .any(|(_, value)| self.has_binding(value, seen)),
            NodeKind::Array(items) => items.iter().any(|item| self.has_binding(item, seen)),
            _ => false,
        }
    }

    /// Whether `node` is a `$path`, directly or through variables.
    fn resolves_to_path(&self, node: &Node) -> bool {
        let mut node = node;
        let mut seen = Vec::new();
        while let NodeKind::Variable(name) = &node.kind {
            match self.scope.symbols.get(name) {
                Some(value) if !seen.contains(&name) => {
                    seen.push(name);
                    node = value;
                }
                _ => return false,
            }
        }
        matches!(node.kind, NodeKind::Path(_))
    }

    /// Compiles a `?check(...)` into its condition and message.
    ///
    /// The check's first parameter, when named `value`, takes the bound
    /// value of the component unless the check names a binding itself. A
    /// string past the last parameter, or at a numeric or boolean one, is
    /// the message. A `{catalogId: ...}` argument names the check's catalog.
    pub(super) fn check(&mut self, check: &'a Check) -> Result<(Value, String), ExpressError> {
        let mut explicit: Vec<&'a Node> = Vec::new();
        let mut catalog_id: Option<&'a Node> = None;
        for arg in &check.args {
            let NodeKind::Map(entries) = &arg.kind else {
                explicit.push(arg);
                continue;
            };
            let Some((_, id)) = entries.iter().find(|(key, _)| key == "catalogId") else {
                explicit.push(arg);
                continue;
            };
            if let Some((key, _)) = entries.iter().find(|(key, _)| key != "catalogId") {
                return Err(ExpressError::validation(format!(
                    "the options of check ?{} take only catalogId, not {key}",
                    check.name
                ))
                .at(arg.pos));
            }
            if matches!(id.kind, NodeKind::Literal(Value::Null)) {
                return Err(ExpressError::validation(format!(
                    "the catalogId of check ?{} must be a string, not null",
                    check.name
                ))
                .at(id.pos));
            }
            if catalog_id.is_some_and(|known| known.kind != id.kind) {
                return Err(ExpressError::validation(format!(
                    "check ?{} names two different catalogs",
                    check.name
                ))
                .at(arg.pos));
            }
            catalog_id = Some(id);
        }
        let (catalog, write_catalog) =
            self.express.resolve(Kind::Check, &check.name, catalog_id)?;
        let sig = &self.signatures(catalog).functions[&check.name];
        let mut message = default_check_message(&check.name);
        let mut args = Map::new();
        let explicit_binding = explicit
            .first()
            .is_some_and(|n| matches!(n.kind, NodeKind::Path(_)));
        let mut start = 0;
        if sig.params.first().is_some_and(|p| p == "value") && !explicit_binding {
            if let Some(path) = self.state.value_path.clone() {
                args.insert("value".to_owned(), path);
                start = 1;
            }
        }
        for (index, node) in explicit.iter().enumerate() {
            let Some(param) = sig.params.get(index + start) else {
                if let Some(text) = node.as_str() {
                    text.clone_into(&mut message);
                }
                continue;
            };
            let typed = sig
                .schema(param)
                .and_then(|s| s.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|t| matches!(t, "integer" | "number" | "boolean"));
            if let (Some(text), true) = (node.as_str(), typed) {
                text.clone_into(&mut message);
                break;
            }
            if matches!(node.kind, NodeKind::Skipped) {
                continue;
            }
            let value = self
                .value(node, &Slot::plain())
                .map_err(|e| e.at(node.pos))?;
            args.insert(param.clone(), value);
        }
        let mut condition = Map::new();
        condition.insert("@call".to_owned(), Value::String(check.name.clone()));
        if let Some(id) = write_catalog {
            condition.insert("catalogId".to_owned(), Value::String(id));
        }
        condition.insert("args".to_owned(), Value::Object(args));
        Ok((Value::Object(condition), message))
    }
}

/// Pairs positional arguments with the parameters they fill. A number
/// at a parameter that takes none moves to the one later parameter
/// that does, when it is the last positional argument and every
/// parameter it passes is an optional enum.
fn positional_pairs<'a>(
    signatures: &Signatures,
    sig: &ComponentSignature,
    name: &str,
    id: &str,
    positional: &[&'a Node],
    kwargs: &[(&str, &'a Node)],
) -> Result<Vec<(String, &'a Node)>, ExpressError> {
    let params: Vec<&String> = sig
        .params
        .iter()
        .filter(|p| *p != "checks" || sig.custom_envelope.contains("checks"))
        .collect();
    let required = |p: &str| sig.required.iter().any(|r| r == p);
    let takes_number = |p: &str| sig.schema(p).is_some_and(|s| signatures.accepts_number(s));
    let optional_enum =
        |p: &str| !required(p) && sig.schema(p).and_then(signature::find_enum).is_some();
    let mut pairs = Vec::new();
    let mut at = 0;
    for (index, node) in positional.iter().enumerate() {
        let Some(param) = params.get(at) else {
            let list: Vec<&str> = params.iter().map(|p| p.as_str()).collect();
            return Err(ExpressError::validation(format!(
                "{name} takes at most {} positional arguments ({}), but component {id} passes {}",
                params.len(),
                list.join(", "),
                positional.len()
            ))
            .at(node.pos));
        };
        if is_number(node) && !takes_number(param) {
            let last = index + 1 == positional.len();
            let later: Vec<usize> = (at + 1..params.len())
                .filter(|&i| !kwargs.iter().any(|(k, _)| k == params[i]) && takes_number(params[i]))
                .collect();
            let moved = match later.as_slice() {
                [target] if last && params[at..*target].iter().all(|p| optional_enum(p)) => {
                    at = *target;
                    true
                }
                _ => false,
            };
            if !moved && !required(param) {
                return Err(ExpressError::validation(format!(
                    "positional argument {} of {name} is a number, which optional property {param} does not take",
                    index + 1
                ))
                .at(node.pos));
            }
        }
        pairs.push((params[at].clone(), *node));
        at += 1;
    }
    Ok(pairs)
}

/// Fails when `component` lacks a property its catalog requires. Checks the
/// envelope holds are never required of the component itself.
fn check_required(
    sig: &ComponentSignature,
    name: &str,
    component: &Map<String, Value>,
    skipped: &HashSet<String>,
) -> Result<(), ExpressError> {
    for required in &sig.required {
        let envelope_checks = matches!(required.as_str(), "checks" | "@checks")
            && !sig.custom_envelope.contains("checks");
        if !component.contains_key(required) && !envelope_checks {
            let skipped = if skipped.contains(required) {
                " ('_' skips it)"
            } else {
                ""
            };
            return Err(ExpressError::validation(format!(
                "component {name} is missing its required property {required}{skipped}"
            )));
        }
    }
    Ok(())
}

/// The message a check gets when none is written: its name, capitalized
/// the way the reference SDKs do, and "check failed".
pub(crate) fn default_check_message(name: &str) -> String {
    let mut chars = name.chars();
    let first: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let rest: String = chars.as_str().to_lowercase();
    format!("{first}{rest} check failed")
}

/// Whether `node` is a check or a non-empty list of checks, which then go
/// to `checks` whatever their position.
fn collect_checks<'a>(node: &'a Node, checks: &mut Vec<(&'a Check, Position)>) -> bool {
    match &node.kind {
        NodeKind::Check(check) => {
            checks.push((check, node.pos));
            true
        }
        NodeKind::Array(items)
            if !items.is_empty() && items.iter().all(|i| matches!(i.kind, NodeKind::Check(_))) =>
        {
            for item in items {
                if let NodeKind::Check(check) = &item.kind {
                    checks.push((check, item.pos));
                }
            }
            true
        }
        _ => false,
    }
}

fn is_number(node: &Node) -> bool {
    matches!(node.kind, NodeKind::Literal(Value::Number(_)))
}

fn unknown_property(component: &str, prop: &str, sig: &ComponentSignature) -> ExpressError {
    let mut accepted: Vec<&str> = sig.params.iter().map(String::as_str).collect();
    accepted.sort_unstable();
    let help = if accepted.is_empty() {
        format!("component {component} has no properties")
    } else {
        format!("component {component} takes {}", accepted.join(", "))
    };
    ExpressError::validation(format!(
        "{prop} is not a property of component {component}; {help}"
    ))
}
