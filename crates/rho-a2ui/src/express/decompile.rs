//! Writing A2UI v1.0 messages back as Express, so that a prompt example
//! authored as messages reaches the model in the notation it writes.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde_json::{Map, Value};

use super::compile::default_check_message;
use super::error::ExpressError;
use super::signature::{self, ComponentSignature, FunctionSignature, SlotKind};
use super::{Express, DEFAULT_SURFACE_ID};
use crate::ident::is_identifier;

/// Words that cannot name a variable.
const RESERVED_WORDS: [&str; 4] = ["_", "true", "false", "null"];

const SURFACE_OPERATIONS: [&str; 4] = [
    "createSurface",
    "updateComponents",
    "updateDataModel",
    "deleteSurface",
];

impl Express {
    /// Writes messages back as one Express block that compiles to them.
    ///
    /// Split surface messages are merged first: an `updateComponents` or a
    /// root `updateDataModel` that follows a `createSurface` for the same
    /// surface folds into it, as the compiler would emit it. Every other
    /// message keeps its place.
    ///
    /// # Errors
    ///
    /// An [`ErrorKind::Validation`](super::ErrorKind::Validation) error
    /// when a message names a catalog, component or function the catalogs
    /// lack, a component id that is not a UAX #31 identifier, or a value
    /// Express cannot write, such as a check whose condition is no call.
    pub fn decompile(&self, messages: &[Value]) -> Result<String, ExpressError> {
        let mut blocks = Vec::new();
        let mut calls = 0;
        for (index, item) in coalesce(messages).into_iter().enumerate() {
            let block = if item.message.get("callRendererFunction").is_some() {
                calls += 1;
                self.renderer_call(&item.message, calls)?
            } else {
                Writer { express: self }.item(&item, index)?
            };
            if !block.is_empty() {
                blocks.push(block);
            }
        }
        Ok(blocks.join("\n"))
    }

    fn renderer_call(&self, message: &Value, index: usize) -> Result<String, ExpressError> {
        let body = &message["callRendererFunction"];
        let function = body.get("callFunction").cloned().unwrap_or_default();
        let sole = self.sole_catalog();
        let mut text =
            Writer { express: self }.call(&function, &HashMap::new(), sole.as_deref())?;
        if let Some(id) = body.get("functionCallId").and_then(Value::as_str) {
            if id != format!("call_{index}") {
                text.pop();
                if !text.ends_with('(') {
                    text.push_str(", ");
                }
                let _ = write!(text, "functionCallId={})", string_literal(id));
            }
        }
        Ok(text)
    }

    /// The id of the only catalog, or `None` when there are several.
    fn sole_catalog(&self) -> Option<String> {
        (!self.is_multi_catalog()).then(|| self.catalogs[0].id().to_owned())
    }
}

/// A message after coalescing, with the catalog of the `createSurface`
/// earlier in the payload for its surface, if any.
struct Coalesced {
    message: Value,
    surface_catalog: Option<String>,
}

impl Coalesced {
    fn operation(&self) -> Option<&'static str> {
        SURFACE_OPERATIONS
            .into_iter()
            .find(|op| self.message.get(*op).is_some_and(Value::is_object))
    }
}

/// Merges split surface messages into the `createSurface` before them, as
/// long as that does not change what the payload means.
fn coalesce(messages: &[Value]) -> Vec<Coalesced> {
    let mut result: Vec<Value> = Vec::new();
    let mut surfaces: Vec<Option<String>> = Vec::new();
    let mut open: HashMap<String, usize> = HashMap::new();
    for message in messages {
        let op = SURFACE_OPERATIONS
            .into_iter()
            .find(|op| message.get(*op).is_some_and(Value::is_object));
        let Some(op) = op else {
            open.clear();
            result.push(message.clone());
            surfaces.push(None);
            continue;
        };
        let body = &message[op];
        let surface = body
            .get("surfaceId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if op == "createSurface" {
            if let Some(surface) = &surface {
                open.insert(surface.clone(), result.len());
            }
            result.push(message.clone());
            surfaces.push(surface);
            continue;
        }
        if let Some(index) = surface.as_ref().and_then(|s| open.get(s)).copied() {
            let create = &mut result[index]["createSurface"];
            if op == "updateComponents" && create.get("components").is_none() {
                create["components"] = body
                    .get("components")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new()));
                continue;
            }
            let root = body
                .get("path")
                .is_none_or(|p| p.is_null() || p == "" || p == "/");
            if op == "updateDataModel" && root && body.get("value").is_some_and(Value::is_object) {
                create["dataModel"] = body["value"].clone();
                continue;
            }
            if let Some(surface) = &surface {
                open.remove(surface);
            }
        }
        if op == "deleteSurface" {
            if let Some(surface) = &surface {
                open.remove(surface);
            }
        }
        result.push(message.clone());
        surfaces.push(surface);
    }
    let mut catalogs: HashMap<String, String> = HashMap::new();
    result
        .into_iter()
        .zip(surfaces)
        .map(|(message, surface)| {
            if let (Some(surface), Some(create)) = (&surface, message.get("createSurface")) {
                match create
                    .get("catalogId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    Some(id) => catalogs.insert(surface.clone(), id.to_owned()),
                    None => catalogs.remove(surface),
                };
            }
            let surface_catalog = surface.and_then(|s| catalogs.get(&s).cloned());
            Coalesced {
                message,
                surface_catalog,
            }
        })
        .collect()
}

/// Writes one message at a time.
struct Writer<'e> {
    express: &'e Express,
}

type VarNames = HashMap<String, String>;

impl Writer<'_> {
    /// Writes one message, the `index`-th of the payload: the first may leave
    /// out a `surface(...)` line for the default surface.
    fn item(&self, item: &Coalesced, index: usize) -> Result<String, ExpressError> {
        let message = &item.message;
        if let Some(body) = message.get("deleteSurface") {
            let surface = body
                .get("surfaceId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Ok(format!("deleteSurface({})", string_literal(surface)));
        }
        match item.operation() {
            Some("updateComponents" | "updateDataModel") => return self.update(item, index),
            Some("createSurface") => {}
            _ => return Ok(String::new()),
        }
        let create = &message["createSurface"];
        let surface = create
            .get("surfaceId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SURFACE_ID);
        let named = create
            .get("catalogId")
            .and_then(Value::as_str)
            .or(item.surface_catalog.as_deref());
        let catalog = self.surface_catalog(named)?;
        let send_data_model = create.get("sendDataModel").and_then(Value::as_bool);
        let extensions = create
            .pointer("/metadata/extensions")
            .filter(|e| e.is_object());
        let mut lines = Vec::new();
        if !(index == 0
            && surface == DEFAULT_SURFACE_ID
            && send_data_model.is_none()
            && extensions.is_none())
        {
            let mut header = format!("surface({}", string_literal(surface));
            if let Some(flag) = send_data_model {
                let _ = write!(header, ", sendDataModel={flag}");
            }
            if let Some(extensions) = extensions {
                let extensions =
                    self.value(extensions, &VarNames::new(), Ref::No, catalog.as_deref())?;
                let _ = write!(header, ", extensions={extensions}");
            }
            header.push(')');
            lines.push(header);
        }
        if let Some(Value::Object(model)) = create.get("dataModel") {
            let mut leaves = Vec::new();
            flatten(&Value::Object(model.clone()), String::new(), &mut leaves);
            leaves.sort_by(|a, b| a.0.cmp(&b.0));
            for (path, value) in leaves {
                let value = self.value(&value, &VarNames::new(), Ref::No, catalog.as_deref())?;
                lines.push(format!("${path} = {value}"));
            }
        }
        let components = create
            .get("components")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        lines.extend(self.components(components, catalog.as_deref())?);
        Ok(lines.join("\n"))
    }

    fn update(&self, item: &Coalesced, index: usize) -> Result<String, ExpressError> {
        let op = item.operation().unwrap_or("updateDataModel");
        let body = &item.message[op];
        let surface = body
            .get("surfaceId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SURFACE_ID);
        let catalog = self.surface_catalog(item.surface_catalog.as_deref())?;
        let components = match (op, body.get("components").and_then(Value::as_array)) {
            ("updateComponents", Some(components)) => components.as_slice(),
            _ => &[],
        };
        let has_root = components.iter().any(|c| c["id"] == "root");
        let mut lines = Vec::new();
        if has_root {
            lines.push(format!("surface({}, update=true)", string_literal(surface)));
        } else if !(index == 0 && surface == DEFAULT_SURFACE_ID) {
            lines.push(format!("surface({})", string_literal(surface)));
        }
        if op == "updateComponents" {
            lines.extend(self.components(components, catalog.as_deref())?);
        } else {
            let path = body
                .get("path")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .unwrap_or("/");
            let value = body.get("value").cloned().unwrap_or(Value::Null);
            let value = self.value(&value, &VarNames::new(), Ref::No, catalog.as_deref())?;
            lines.push(format!("{} = {value}", path_literal(path)));
        }
        Ok(lines.join("\n"))
    }

    /// The catalog of a surface the payload names: the only catalog when
    /// there is one, after checking that the payload names no other.
    fn surface_catalog(&self, named: Option<&str>) -> Result<Option<String>, ExpressError> {
        let Some(sole) = self.express.sole_catalog() else {
            return Ok(named.map(str::to_owned));
        };
        if let Some(named) = named {
            self.express.catalog_for(named)?;
        }
        Ok(Some(sole))
    }

    fn components(
        &self,
        components: &[Value],
        surface: Option<&str>,
    ) -> Result<Vec<String>, ExpressError> {
        let ids: Vec<&str> = components
            .iter()
            .map(|c| c.get("id").and_then(Value::as_str).unwrap_or_default())
            .collect();
        if let Some(bad) = ids.iter().find(|id| !is_identifier(id)) {
            return Err(ExpressError::validation(format!(
                "component id {bad:?} must be a valid UAX #31 identifier"
            )));
        }
        let names = variable_names(&ids);
        components
            .iter()
            .zip(&ids)
            .map(|(component, id)| self.component(component, id, &names, surface))
            .collect()
    }

    fn component(
        &self,
        c: &Value,
        id: &str,
        names: &VarNames,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        let name = c
            .get("component")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let override_id = self.component_catalog(c, name, surface)?;
        let (catalog, write_catalog) =
            self.resolve(Lookup::Component, name, override_id.as_deref().or(surface))?;
        let sig = &self.express.signatures[catalog].components[name];
        let mut args: Vec<String> = Vec::new();
        for (index, param) in sig.params.iter().enumerate() {
            if param == "checks" && sig.takes_envelope_checks() {
                let checks = envelope_value(c, "checks", sig).and_then(Value::as_array);
                match checks.filter(|checks| !checks.is_empty()) {
                    Some(checks) => args.push(self.checks(checks, c, names, surface)?),
                    None => args.push("_".to_owned()),
                }
                continue;
            }
            if let Some(value) = c.get(param) {
                let reference = if sig.schema(param).is_some_and(is_reference) {
                    Ref::Yes
                } else {
                    Ref::No
                };
                args.push(self.value(value, names, reference, surface)?);
            } else {
                let later = sig.params[index + 1..].iter().any(|p| {
                    (p != "checks" || sig.custom_envelope.contains("checks")) && c.get(p).is_some()
                });
                if later {
                    args.push("_".to_owned());
                }
            }
        }
        strip_placeholders(&mut args);
        if sig.custom_envelope.contains("checks") && sig.checkable {
            if let Some(checks) = envelope_value(c, "checks", sig)
                .and_then(Value::as_array)
                .filter(|c| !c.is_empty())
            {
                args.push(self.checks(checks, c, names, surface)?);
            }
        }
        if let Some(id) = write_catalog {
            args.push(format!("catalogId={}", string_literal(&id)));
        }
        let variable = &names[id];
        if variable != id {
            args.push(format!("id={}", string_literal(id)));
        }
        if let Some(extensions) = envelope_value(c, "metadata", sig)
            .and_then(|m| m.get("extensions"))
            .filter(|e| e.is_object())
        {
            args.push(format!(
                "extensions={}",
                self.value(extensions, names, Ref::No, surface)?
            ));
        }
        Ok(format!("{variable} = {name}({})", args.join(", ")))
    }

    fn checks(
        &self,
        checks: &[Value],
        c: &Value,
        names: &VarNames,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        let written = checks
            .iter()
            .map(|rule| self.check(rule, c, names, surface))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match written.as_slice() {
            [one] => one.clone(),
            _ => format!("[{}]", written.join(", ")),
        })
    }

    /// The catalog a component names for itself, or `None`. A bare
    /// `catalogId`, from payloads written before v1.0's `@catalogId`, counts
    /// unless the component declares a `catalogId` property of its own.
    fn component_catalog(
        &self,
        c: &Value,
        name: &str,
        surface: Option<&str>,
    ) -> Result<Option<String>, ExpressError> {
        if let Some(id) = c.get("@catalogId") {
            return match id.as_str() {
                Some(id) if !id.is_empty() => Ok(Some(id.to_owned())),
                _ => Err(ExpressError::validation(format!(
                    "component {:?} has an invalid @catalogId: {id}",
                    c.get("id").and_then(Value::as_str).unwrap_or("<unknown>")
                ))),
            };
        }
        let Some(legacy) = c.get("catalogId").and_then(Value::as_str) else {
            return Ok(None);
        };
        let declares_own = |sig: &ComponentSignature| sig.custom_envelope.contains("catalogId");
        let home = surface
            .and_then(|id| self.express.catalog_index(id))
            .and_then(|i| self.express.signatures[i].components.get(name));
        let own = if let Some(sig) = home {
            declares_own(sig)
        } else {
            let declaring: Vec<&ComponentSignature> = self
                .express
                .signatures
                .iter()
                .filter_map(|s| s.components.get(name))
                .collect();
            !declaring.is_empty() && declaring.into_iter().all(declares_own)
        };
        Ok((!own).then(|| legacy.to_owned()))
    }

    /// The catalog a component or call belongs to, and the catalog id to
    /// write: only when several catalogs are active and a lookup by name
    /// would not find this one.
    fn resolve(
        &self,
        lookup: Lookup,
        name: &str,
        catalog: Option<&str>,
    ) -> Result<(usize, Option<String>), ExpressError> {
        let label = lookup.label();
        let Some(actual) = catalog else {
            return Err(ExpressError::validation(format!(
                "{label} {name} names no catalogId, and its surface has no catalog"
            )));
        };
        let index = self.express.catalog_for(actual)?;
        let defines = |i: usize| match lookup {
            Lookup::Component => self.express.signatures[i].components.contains_key(name),
            Lookup::Function => self.express.signatures[i].functions.contains_key(name),
        };
        if !defines(index) {
            return Err(ExpressError::validation(format!(
                "unknown {label} {name}: catalog {actual:?} does not define it"
            )));
        }
        if !self.express.is_multi_catalog() {
            return Ok((index, None));
        }
        let found: Vec<usize> = (0..self.express.catalogs.len())
            .filter(|&i| defines(i))
            .collect();
        Ok((index, (found != [index]).then(|| actual.to_owned())))
    }

    /// `?name(args..., message, {catalogId: ...})`, leaving out the
    /// component's own bound value and the default message.
    fn check(
        &self,
        rule: &Value,
        c: &Value,
        names: &VarNames,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        let condition = rule.get("condition").unwrap_or(rule);
        let Some(name) = condition.get("@call").and_then(Value::as_str) else {
            return Err(ExpressError::validation(format!(
                "Express cannot write a check whose condition is not a function call: {condition}"
            )));
        };
        let empty = Map::new();
        let args = condition
            .get("args")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let catalog = condition
            .get("catalogId")
            .and_then(Value::as_str)
            .or(surface);
        let (index, write_catalog) = self.resolve(Lookup::Function, name, catalog)?;
        let sig = &self.express.signatures[index].functions[name];
        let bound = c.get("value").filter(|v| v.get("@path").is_some());
        let start = usize::from(
            sig.params.first().is_some_and(|p| p == "value")
                && bound.is_some()
                && args.get("value") == bound,
        );
        let mut written = Vec::new();
        for param in &sig.params[start.min(sig.params.len())..] {
            match args.get(param) {
                Some(value) => written.push(self.value(value, names, Ref::No, surface)?),
                None => written.push("_".to_owned()),
            }
        }
        let message = rule
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if message.is_empty() || message == default_check_message(name) {
            strip_placeholders(&mut written);
        } else {
            written.push(string_literal(message));
        }
        if let Some(id) = write_catalog {
            written.push(format!("{{catalogId: {}}}", string_literal(&id)));
        }
        Ok(if written.is_empty() {
            format!("?{name}")
        } else {
            format!("?{name}({})", written.join(", "))
        })
    }

    /// `name(args..., catalogId=...)`.
    fn call(
        &self,
        function: &Value,
        names: &VarNames,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        let name = function
            .get("@call")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let empty = Map::new();
        let args = function
            .get("args")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let (sig, write_catalog): (&FunctionSignature, Option<String>) = if name.starts_with('@') {
            let Some(sig) = self.express.system.get(name) else {
                return Err(ExpressError::validation(format!(
                    "Unknown system function {name}"
                )));
            };
            if function.get("catalogId").is_some() {
                return Err(ExpressError::validation(format!(
                    "system function {name} is defined by the protocol, not a catalog, so it takes no catalogId"
                )));
            }
            (sig, None)
        } else {
            let catalog = function
                .get("catalogId")
                .and_then(Value::as_str)
                .or(surface);
            let (index, write_catalog) = self.resolve(Lookup::Function, name, catalog)?;
            (
                &self.express.signatures[index].functions[name],
                write_catalog,
            )
        };
        let mut written = Vec::new();
        for param in &sig.params {
            match args.get(param) {
                Some(value) => written.push(self.value(value, names, Ref::No, surface)?),
                None => written.push("_".to_owned()),
            }
        }
        strip_placeholders(&mut written);
        if let Some(id) = write_catalog {
            written.push(format!("catalogId={}", string_literal(&id)));
        }
        Ok(format!("{name}({})", written.join(", ")))
    }

    fn value(
        &self,
        value: &Value,
        names: &VarNames,
        reference: Ref,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        match value {
            Value::Object(map) => self.object(map, names, reference, surface),
            Value::Array(items) => {
                let written = items
                    .iter()
                    .map(|item| self.value(item, names, reference, surface))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("[{}]", written.join(", ")))
            }
            Value::String(text) if reference == Ref::Yes => Ok(reference_literal(text, names)),
            Value::String(text) => Ok(string_literal(text)),
            Value::Bool(flag) => Ok(flag.to_string()),
            Value::Number(number) => number_literal(number),
            Value::Null => Ok("null".to_owned()),
        }
    }

    fn object(
        &self,
        map: &Map<String, Value>,
        names: &VarNames,
        reference: Ref,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        if let (Some(component), Some(Value::String(path))) =
            (map.get("componentId"), map.get("path"))
        {
            let component = match component {
                Value::String(id) => reference_literal(id, names),
                other => string_literal(&other.to_string()),
            };
            return Ok(format!("_template({}, {component})", path_literal(path)));
        }
        if let (Some(Value::String(path)), 1) = (map.get("@path"), map.len()) {
            return Ok(path_literal(path));
        }
        if let Some(Value::Object(event)) = map.get("event") {
            return self.event(event, names, surface);
        }
        if let Some(function) = map.get("functionCall").filter(|f| f.is_object()) {
            return self.call(function, names, surface);
        }
        if map.get("@call").is_some_and(Value::is_string) {
            return self.call(&Value::Object(map.clone()), names, surface);
        }
        let entries = map
            .iter()
            .map(|(key, value)| {
                let entry_ref = if reference == Ref::Yes || key == "child" || key == "componentId" {
                    Ref::Yes
                } else {
                    Ref::No
                };
                Ok(format!(
                    "{}: {}",
                    key_literal(key),
                    self.value(value, names, entry_ref, surface)?
                ))
            })
            .collect::<Result<Vec<_>, ExpressError>>()?;
        Ok(format!("{{{}}}", entries.join(", ")))
    }

    fn event(
        &self,
        event: &Map<String, Value>,
        names: &VarNames,
        surface: Option<&str>,
    ) -> Result<String, ExpressError> {
        let name = event
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut args = vec![string_literal(name)];
        if let Some(context) = event.get("context") {
            let empty = Map::new();
            let entries = context
                .as_object()
                .unwrap_or(&empty)
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        key_literal(key),
                        self.value(value, names, Ref::No, surface)?
                    ))
                })
                .collect::<Result<Vec<_>, ExpressError>>()?;
            args.push(format!("{{{}}}", entries.join(", ")));
        }
        if let Some(message) = event.get("userMessage").filter(|m| !m.is_null()) {
            args.push(format!(
                "userMessage={}",
                self.value(message, names, Ref::No, surface)?
            ));
        }
        Ok(format!("Event({})", args.join(", ")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lookup {
    Component,
    Function,
}

impl Lookup {
    fn label(self) -> &'static str {
        match self {
            Self::Component => "component",
            Self::Function => "function",
        }
    }
}

/// Whether a string value names a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ref {
    Yes,
    No,
}

/// An envelope mixin's value: `@name`, else a bare `name` from before
/// v1.0's `@` keys, unless the component declares `name` itself.
fn envelope_value<'v>(c: &'v Value, name: &str, sig: &ComponentSignature) -> Option<&'v Value> {
    c.get(format!("@{name}")).or_else(|| {
        if sig.custom_envelope.contains(name) {
            None
        } else {
            c.get(name)
        }
    })
}

/// Whether a property schema names a component or a list of them.
fn is_reference(schema: &Value) -> bool {
    matches!(
        signature::slot_kind(schema),
        Some(SlotKind::Child | SlotKind::ChildList)
    ) || schema.get("type").and_then(Value::as_str) == Some("array")
        && schema.get("items").is_some_and(is_reference)
}

fn is_variable_name(name: &str) -> bool {
    is_identifier(name) && !RESERVED_WORDS.contains(&name)
}

/// The variable each component id is written under: the id itself when it
/// can name a variable, else a derived name no other id takes, written
/// with `id=` so it compiles back to the id.
fn variable_names(ids: &[&str]) -> VarNames {
    let mut names: VarNames = ids
        .iter()
        .filter(|id| is_variable_name(id))
        .map(|id| ((*id).to_owned(), (*id).to_owned()))
        .collect();
    let mut taken: Vec<String> = names.values().cloned().collect();
    for id in ids {
        if names.contains_key(*id) {
            continue;
        }
        let mut base: String = id
            .chars()
            .map(|c| {
                if unicode_ident::is_xid_continue(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        while !is_variable_name(&base) {
            base.insert(0, '_');
        }
        let mut name = base.clone();
        let mut suffix = 2;
        while taken.contains(&name) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        taken.push(name.clone());
        names.insert((*id).to_owned(), name);
    }
    names
}

/// A reference to a component by id: its variable when the block defines
/// it, a bare id when that reads back as the id, else a quoted id.
fn reference_literal(id: &str, names: &VarNames) -> String {
    if let Some(name) = names.get(id) {
        return name.clone();
    }
    if is_variable_name(id) && !names.values().any(|n| n == id) {
        return id.to_owned();
    }
    string_literal(id)
}

/// The leaves of a data model, keyed by path: maps are walked, anything
/// else, an empty map included, is a leaf.
fn flatten(value: &Value, path: String, leaves: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                flatten(child, format!("{path}/{key}"), leaves);
            }
        }
        other => leaves.push((path, other.clone())),
    }
}

fn strip_placeholders(args: &mut Vec<String>) {
    while args.last().is_some_and(|a| a == "_") {
        args.pop();
    }
}

/// A data path as Express writes it: `/a/b` is `$/a/b`.
fn path_literal(path: &str) -> String {
    format!("${path}")
}

fn key_literal(key: &str) -> String {
    if is_variable_name(key) {
        key.to_owned()
    } else {
        string_literal(key)
    }
}

/// A string in its most readable form: triple-quoted across lines, raw
/// when it carries backslashes, else escaped.
pub(crate) fn string_literal(text: &str) -> String {
    let newline = text.contains(['\n', '\r']);
    let tab = text.contains('\t');
    let quote = text.contains('"');
    let backslash = text.contains('\\');
    if newline && !text.ends_with('"') && !text.contains("\"\"\"") {
        if backslash && !tab {
            return format!("r\"\"\"{text}\"\"\"");
        }
        return format!(
            "\"\"\"{}\"\"\"",
            text.replace('\\', "\\\\").replace('\t', "\\t")
        );
    }
    if backslash && !newline && !tab && !quote {
        return format!("r\"{text}\"");
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number as ECMAScript's `Number.prototype.toString` writes it: plain
/// decimal for `1e-6 <= |v| < 1e21`, else `d.ddde+x`. An integer is its
/// digits; an integral double keeps a `.0`, so it reads back as a double.
fn number_literal(number: &serde_json::Number) -> Result<String, ExpressError> {
    if number.is_i64() || number.is_u64() {
        return Ok(number.to_string());
    }
    let value = number.as_f64().unwrap_or_default();
    if !value.is_finite() {
        return Err(ExpressError::validation(format!(
            "the number {value} has no Express notation"
        )));
    }
    let negative = value.is_sign_negative();
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let n = exponent.parse::<i32>().unwrap_or(0) + 1;
    let k = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let text = if k <= n && n <= 21 {
        format!(
            "{digits}{}.0",
            "0".repeat(usize::try_from(n - k).unwrap_or(0))
        )
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(0);
        format!("{}.{}", &digits[..split], &digits[split..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let fraction = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        format!("{}{fraction}e{:+}", &digits[..1], n - 1)
    };
    Ok(if negative { format!("-{text}") } else { text })
}

#[cfg(test)]
#[path = "decompile_tests.rs"]
mod tests;
