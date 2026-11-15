//! Compiling values: literals, bindings, variables, calls, events and
//! templates, and the standalone renderer function call.

use serde_json::{json, Map, Value};

use super::{component_id, Compiler, Kind, Slot, MAX_VALUES, MAX_VALUE_DEPTH};
use crate::express::error::ExpressError;
use crate::express::parser::{Call, Node, NodeKind};
use crate::express::signature::{self, FunctionSignature, SlotKind};
use crate::express::VERSION;
use crate::ident::is_identifier;

impl<'e, 'a> Compiler<'e, 'a> {
    /// Compiles `node` in `slot`, bounding how deep and how much variable
    /// substitution may expand.
    pub(super) fn value(&mut self, node: &'a Node, slot: &Slot<'_>) -> Result<Value, ExpressError> {
        self.state.values += 1;
        if self.state.values > MAX_VALUES {
            return Err(ExpressError::validation(format!(
                "the block expands to more than {MAX_VALUES} values once its variables are substituted"
            ))
            .at(node.pos));
        }
        self.state.depth += 1;
        let result = if self.state.depth > MAX_VALUE_DEPTH {
            Err(ExpressError::validation(format!(
                "values nest deeper than {MAX_VALUE_DEPTH} levels once variables are substituted"
            )))
        } else {
            self.value_kind(node, slot)
        };
        self.state.depth -= 1;
        result.map_err(|e| e.at(node.pos))
    }

    fn value_kind(&mut self, node: &'a Node, slot: &Slot<'_>) -> Result<Value, ExpressError> {
        match &node.kind {
            NodeKind::Literal(value) => Ok(value.clone()),
            NodeKind::Path(path) => Ok(json!({ "@path": path })),
            NodeKind::Variable(name) => self.variable(name, slot),
            NodeKind::Check(check) => self.check(check).map(|(condition, _)| condition),
            NodeKind::Call(call) => self.call(call, slot),
            NodeKind::Skipped => Err(ExpressError::validation(
                "'_' can only stand in for an argument of a call",
            )),
            NodeKind::Map(entries) => self.map(entries, slot),
            NodeKind::Array(items) => self.list(items, slot),
        }
    }

    fn variable(&mut self, name: &str, slot: &Slot<'_>) -> Result<Value, ExpressError> {
        let Some(value) = self.scope.symbols.get(name) else {
            if slot.component_ref {
                // A component from an earlier turn, named by its id.
                return Ok(Value::String(name.to_owned()));
            }
            return Err(ExpressError::validation(format!(
                "the variable {name} is not assigned in this surface's statements"
            )));
        };
        if let NodeKind::Call(call) = &value.kind {
            if self
                .express
                .defines_component(&call.name, call.kwarg("catalogId"))
            {
                return component_id(name, call).map(Value::String);
            }
        }
        if self.state.resolving.iter().any(|r| r == name) {
            return Err(ExpressError::validation(format!(
                "the variable {name} refers to itself through its own value"
            )));
        }
        self.state.resolving.push(name.to_owned());
        let inner = Slot {
            parent: None,
            index: None,
            ..slot.clone()
        };
        let result = self.value(value, &inner);
        self.state.resolving.pop();
        result
    }

    fn map(
        &mut self,
        entries: &'a [(String, Node)],
        slot: &Slot<'_>,
    ) -> Result<Value, ExpressError> {
        let mut map = Map::new();
        for (key, value) in entries {
            let schema = slot.schema.as_ref().and_then(|(schema, catalog)| {
                Some((
                    self.signatures(*catalog).property_schema(schema, key)?,
                    *catalog,
                ))
            });
            let kind = schema.as_ref().and_then(|(s, _)| signature::slot_kind(s));
            let entry = Slot {
                action: slot.action
                    || schema
                        .as_ref()
                        .is_some_and(|(s, _)| signature::is_action(s)),
                parent: None,
                index: None,
                component_ref: kind == Some(SlotKind::Child),
                list_refs: kind == Some(SlotKind::ChildList)
                    && matches!(value.kind, NodeKind::Array(_)),
                schema,
            };
            let compiled = self.value(value, &entry)?;
            map.insert(key.clone(), compiled);
        }
        Ok(Value::Object(map))
    }

    fn list(&mut self, items: &'a [Node], slot: &Slot<'_>) -> Result<Value, ExpressError> {
        let schema = slot.schema.as_ref().and_then(|(schema, catalog)| {
            Some((self.signatures(*catalog).item_schema(schema)?, *catalog))
        });
        let kind = schema.as_ref().and_then(|(s, _)| signature::slot_kind(s));
        let action = slot.action
            || schema
                .as_ref()
                .is_some_and(|(s, _)| signature::is_action(s));
        let mut compiled = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let item_slot = Slot {
                action,
                parent: slot.parent,
                index: Some(index),
                component_ref: slot.list_refs || kind == Some(SlotKind::Child),
                list_refs: false,
                schema: schema.clone(),
            };
            compiled.push(self.value(item, &item_slot)?);
        }
        Ok(Value::Array(compiled))
    }

    /// A call: a template, an event, a system function, an inline
    /// component, or a catalog function.
    fn call(&mut self, call: &'a Call, slot: &Slot<'_>) -> Result<Value, ExpressError> {
        match call.name.as_str() {
            "_template" => return self.template(call),
            "Event" => return self.event(call),
            name if name.starts_with('@') => return self.system_call(call, slot),
            _ => {}
        }
        let catalog_id = call.kwarg("catalogId");
        if self.express.defines_component(&call.name, catalog_id) {
            return self.inline_component(call, slot);
        }
        let (catalog, write_catalog) =
            self.express
                .resolve(Kind::Function, &call.name, catalog_id)?;
        let sig = &self.signatures(catalog).functions[&call.name];
        let args = self.call_args(sig, call, &["catalogId"])?;
        let mut compiled = Map::new();
        compiled.insert("@call".to_owned(), Value::String(call.name.clone()));
        if let Some(id) = write_catalog {
            compiled.insert("catalogId".to_owned(), Value::String(id));
        }
        compiled.insert("args".to_owned(), Value::Object(args));
        Ok(wrap_action(Value::Object(compiled), slot))
    }

    /// `_template($path, component)`: a child list generated from a list in
    /// the data model.
    fn template(&mut self, call: &'a Call) -> Result<Value, ExpressError> {
        let (Some(path), Some(component)) = (call.args.first(), call.args.get(1)) else {
            return Err(ExpressError::parse(
                call.args.first().map_or_else(|| crate::express::error::Position { line: 1, column: 1 }, |n| n.pos),
                "_template takes two arguments: the $path of a list and the component each item renders",
            ));
        };
        let bound = self.value(path, &Slot::plain())?;
        let Some(path_text) = bound
            .as_object()
            .filter(|map| map.len() == 1)
            .and_then(|map| map.get("@path"))
            .and_then(Value::as_str)
        else {
            return Err(ExpressError::parse(
                path.pos,
                format!(
                    "the first argument of _template must be a $path, not {}",
                    path.describe()
                ),
            ));
        };
        let path_text = path_text.to_owned();
        let component_slot = Slot {
            component_ref: true,
            ..Slot::plain()
        };
        let component = self.value(component, &component_slot)?;
        Ok(json!({ "path": path_text, "componentId": component }))
    }

    /// `Event(name, context?, userMessage=?, context=?)`.
    fn event(&mut self, call: &'a Call) -> Result<Value, ExpressError> {
        if call.args.len() > 2 {
            return Err(ExpressError::validation(format!(
                "Event takes at most 2 positional arguments (name, context), not {}",
                call.args.len()
            )));
        }
        if let Some((key, node)) = call
            .kwargs
            .iter()
            .find(|(key, _)| !EVENT_KEYWORDS.contains(&key.as_str()))
        {
            return Err(ExpressError::validation(format!(
                "{key} is not an argument of Event; it takes {}",
                EVENT_KEYWORDS.join(", ")
            ))
            .at(node.pos));
        }
        if call.args.len() == 2 && call.kwarg("context").is_some() {
            return Err(ExpressError::validation(
                "the context of Event is given both by position and by keyword",
            ));
        }
        let name = match call.args.first() {
            Some(node) => self.value(node, &Slot::plain())?,
            None => Value::String(String::new()),
        };
        let mut event = Map::new();
        event.insert("name".to_owned(), name);
        if let Some(node) = call.kwarg("userMessage") {
            let message = self.value(node, &Slot::plain())?;
            let dynamic = message.is_string()
                || message
                    .as_object()
                    .is_some_and(|m| m.contains_key("@path") || m.contains_key("@call"));
            if !dynamic {
                return Err(ExpressError::validation(format!(
                    "the userMessage of Event must be a string, a $path or a function call, not {}",
                    node.describe()
                ))
                .at(node.pos));
            }
            event.insert("userMessage".to_owned(), message);
        }
        let context = call.args.get(1).or_else(|| call.kwarg("context"));
        if let Some(node) = context {
            let compiled = self.value(node, &Slot::plain())?;
            let mut context = Map::new();
            match compiled {
                Value::Object(map) => context.extend(map),
                Value::Array(items) => {
                    for item in items {
                        if let Value::Object(map) = item {
                            context.extend(map);
                        }
                    }
                }
                _ => {}
            }
            event.insert("context".to_owned(), Value::Object(context));
        }
        Ok(json!({ "event": event }))
    }

    /// A call to a protocol system function, such as `@index(offset=1)`. It
    /// names no catalog, and leaves out empty arguments.
    fn system_call(&mut self, call: &'a Call, slot: &Slot<'_>) -> Result<Value, ExpressError> {
        let sig = self.system_function(call)?;
        let args = self.call_args(sig, call, &[])?;
        let mut compiled = Map::new();
        compiled.insert("@call".to_owned(), Value::String(call.name.clone()));
        if !args.is_empty() {
            compiled.insert("args".to_owned(), Value::Object(args));
        }
        Ok(wrap_action(Value::Object(compiled), slot))
    }

    /// The system function `call` names, checked.
    pub(super) fn system_function(
        &self,
        call: &Call,
    ) -> Result<&'e FunctionSignature, ExpressError> {
        let Some(sig) = self.express.system.get(&call.name) else {
            let known: Vec<&str> = self.express.system.keys().map(String::as_str).collect();
            return Err(ExpressError::validation(format!(
                "Unknown system function {} in A2UI {VERSION}; the system functions are {}",
                call.name,
                known.join(", ")
            )));
        };
        if call.kwarg("catalogId").is_some() {
            return Err(ExpressError::validation(format!(
                "system function {} is defined by the protocol, not a catalog, so it takes no catalogId",
                call.name
            )));
        }
        Ok(sig)
    }

    /// Maps a function call's arguments to its parameters, leaving out the
    /// reserved keyword arguments `reserved`.
    fn call_args(
        &mut self,
        sig: &FunctionSignature,
        call: &'a Call,
        reserved: &[&str],
    ) -> Result<Map<String, Value>, ExpressError> {
        if call.args.len() > sig.params.len() {
            return Err(ExpressError::validation(format!(
                "{} takes at most {} positional arguments ({}), not {}",
                call.name,
                sig.params.len(),
                sig.params.join(", "),
                call.args.len()
            )));
        }
        let mut args = Map::new();
        for (param, node) in sig.params.iter().zip(&call.args) {
            if matches!(node.kind, NodeKind::Skipped) {
                continue;
            }
            let value = self.value(node, &Slot::plain())?;
            if !value.is_null() {
                args.insert(param.clone(), value);
            }
        }
        for (key, node) in &call.kwargs {
            if reserved.contains(&key.as_str()) {
                continue;
            }
            if !sig.params.contains(key) {
                let help = if sig.params.is_empty() {
                    format!("{} takes no arguments", call.name)
                } else {
                    format!("{} takes {}", call.name, sig.params.join(", "))
                };
                return Err(ExpressError::validation(format!(
                    "{key} is not an argument of function {}; {help}",
                    call.name
                ))
                .at(node.pos));
            }
            if args.contains_key(key) {
                return Err(ExpressError::validation(format!(
                    "argument {key} of function {} is given twice",
                    call.name
                ))
                .at(node.pos));
            }
            if matches!(node.kind, NodeKind::Skipped) {
                continue;
            }
            let value = self.value(node, &Slot::plain())?;
            if !value.is_null() {
                args.insert(key.clone(), value);
            }
        }
        Ok(args)
    }

    /// Hoists a component written inline out after its parent, under the
    /// id `<parent>_<property>` or `<parent>_<property>_<index>`.
    fn inline_component(&mut self, call: &'a Call, slot: &Slot<'_>) -> Result<Value, ExpressError> {
        let name = match (slot.parent, slot.index) {
            (Some((parent, prop)), Some(index)) => format!("{parent}_{prop}_{index}"),
            (Some((parent, prop)), None) => format!("{parent}_{prop}"),
            (None, _) => {
                self.state.inline_counter += 1;
                format!("_inline_{}", self.state.inline_counter)
            }
        };
        let id = component_id(&name, call)?;
        if !self.state.used_ids.insert(id.clone()) {
            return Err(ExpressError::validation(format!(
                "duplicate component id {id:?}: an inline component takes this id, built from its \
                 parent and property, and a variable already holds a component with it"
            )));
        }
        let outer = std::mem::take(&mut self.state.extras);
        let compiled = self.component(&name, call);
        let inner = std::mem::replace(&mut self.state.extras, outer);
        self.state.extras.push(compiled?);
        self.state.extras.extend(inner);
        Ok(Value::String(id))
    }

    /// Compiles `extensions=`: a map of UAX #31 keys holding data, never a
    /// component, an event or a template.
    pub(super) fn extensions(
        &mut self,
        node: &'a Node,
        target: &str,
    ) -> Result<Value, ExpressError> {
        let top = self.resolve_variables(node)?;
        if !matches!(top.kind, NodeKind::Map(_)) {
            return Err(ExpressError::validation(format!(
                "the extensions of {target} must be a map, not {}",
                top.describe()
            ))
            .at(node.pos));
        }
        self.check_extensions(top, target, 0)?;
        let compiled = self.value(node, &Slot::plain())?;
        if let Some(key) = compiled
            .as_object()
            .and_then(|m| m.keys().find(|k| !is_identifier(k)))
        {
            return Err(ExpressError::validation(format!(
                "extension key {key:?} of {target} is not a UAX #31 identifier"
            ))
            .at(node.pos));
        }
        Ok(compiled)
    }

    fn resolve_variables(&self, node: &'a Node) -> Result<&'a Node, ExpressError> {
        let mut node = node;
        let mut seen: Vec<&str> = Vec::new();
        while let NodeKind::Variable(name) = &node.kind {
            if seen.contains(&name.as_str()) {
                return Err(ExpressError::validation(format!(
                    "the variable {name} refers to itself"
                )));
            }
            let Some(value) = self.scope.symbols.get(name) else {
                return Err(ExpressError::validation(format!(
                    "the variable {name} is not assigned in this surface's statements"
                ))
                .at(node.pos));
            };
            seen.push(name);
            node = value;
        }
        Ok(node)
    }

    fn check_extensions(
        &self,
        node: &'a Node,
        target: &str,
        depth: usize,
    ) -> Result<(), ExpressError> {
        if depth > MAX_VALUE_DEPTH {
            return Err(ExpressError::validation("extensions nest too deep"));
        }
        let node = self.resolve_variables(node)?;
        match &node.kind {
            NodeKind::Map(entries) => entries
                .iter()
                .try_for_each(|(_, value)| self.check_extensions(value, target, depth + 1)),
            NodeKind::Array(items) => items
                .iter()
                .try_for_each(|item| self.check_extensions(item, target, depth + 1)),
            NodeKind::Call(call) => {
                let catalog_id = call.kwarg("catalogId");
                if self.express.defines_component(&call.name, catalog_id) {
                    return Err(ExpressError::validation(format!(
                        "the extensions of {target} cannot hold the component {}",
                        call.name
                    ))
                    .at(node.pos));
                }
                let reserved = matches!(
                    call.name.as_str(),
                    "Event" | "_template" | "surface" | "deleteSurface"
                );
                if reserved && catalog_id.is_none() {
                    return Err(ExpressError::validation(format!(
                        "the extensions of {target} cannot hold {}()",
                        call.name
                    ))
                    .at(node.pos));
                }
                call.args
                    .iter()
                    .chain(call.kwargs.iter().map(|(_, n)| n))
                    .try_for_each(|arg| self.check_extensions(arg, target, depth + 1))
            }
            _ => Ok(()),
        }
    }

    /// A standalone call: `callRendererFunction` asking the renderer to run
    /// an agent-callable function now.
    pub(super) fn standalone_call(&mut self, call: &'a Call) -> Result<Value, ExpressError> {
        self.state = super::ScopeState::default();
        self.calls += 1;
        let mut call_id = format!("call_{}", self.calls);
        if let Some(node) = call.kwarg("functionCallId") {
            match node.as_str() {
                Some(id) if !id.is_empty() => id.clone_into(&mut call_id),
                _ => {
                    return Err(ExpressError::validation(format!(
                        "functionCallId must be a non-empty string, not {}",
                        node.describe()
                    ))
                    .at(node.pos))
                }
            }
        }
        if call.name.starts_with('@') {
            return Err(ExpressError::validation(format!(
                "system function {} cannot be a standalone statement: a renderer function call \
                 names a catalog function",
                call.name
            )));
        }
        let catalog_id = call.kwarg("catalogId");
        if self.express.defines_component(&call.name, catalog_id) {
            return Err(ExpressError::validation(format!(
                "{} is a component; a standalone statement calls a function",
                call.name
            )));
        }
        let (catalog, _) = self
            .express
            .resolve(Kind::Function, &call.name, catalog_id)?;
        let sig = &self.signatures(catalog).functions[&call.name];
        if !sig.agent_callable() {
            let why = if sig.requires_user_activation {
                "it runs only on a user's gesture"
            } else {
                "only the renderer may call it"
            };
            let callable = self.express.agent_callable_functions();
            let help = if callable.is_empty() {
                "no active function can be called this way".to_owned()
            } else {
                format!("the functions that can are {}", callable.join(", "))
            };
            return Err(ExpressError::validation(format!(
                "{} cannot be called as a standalone statement, since {why}; {help}",
                call.name
            )));
        }
        let args = self.call_args(sig, call, &["catalogId", "functionCallId"])?;
        Ok(json!({
            "version": VERSION,
            "callRendererFunction": {
                "functionCallId": call_id,
                "callFunction": {
                    "catalogId": self.express.catalogs[catalog].id(),
                    "@call": call.name,
                    "args": args,
                },
            },
        }))
    }
}

/// The keyword arguments `Event(...)` takes.
const EVENT_KEYWORDS: [&str; 2] = ["userMessage", "context"];

/// A function call in an action slot is the action's `functionCall` branch.
fn wrap_action(call: Value, slot: &Slot<'_>) -> Value {
    if slot.action {
        json!({ "functionCall": call })
    } else {
        call
    }
}
