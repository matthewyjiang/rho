//! The prompt that teaches a model Express for a set of catalogs: the
//! syntax, each component and function as a positional signature, and
//! examples written in Express.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

use super::error::ExpressError;
use super::signature::{self, ComponentSignature, FunctionSignature, Signatures, SlotKind};
use super::Express;
use crate::catalog::ReturnType;

/// How the model sets its Express block apart from the rest of its reply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Framing {
    /// The block is all the model writes in this place, such as a tool
    /// argument, so the prompt says nothing of framing.
    #[default]
    Bare,
    /// The block sits in prose between the literal tags `<a2ui>` and
    /// `</a2ui>`, as the Express specification frames it, and examples are
    /// shown in those tags.
    Tagged,
}

/// A message the model may produce, named by the statements that compile
/// to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllowedMessage {
    /// `createSurface`: a block that assigns `root`.
    CreateSurface,
    /// `updateComponents`: components without `root`, or `update=true`.
    UpdateComponents,
    /// `updateDataModel`: `$/path = value` outside a surface creation.
    UpdateDataModel,
    /// `deleteSurface`: `deleteSurface("id")`.
    DeleteSurface,
    /// `callRendererFunction`: a standalone function call.
    CallRendererFunction,
}

impl AllowedMessage {
    /// The message named as on the wire, such as `createSurface`.
    pub fn from_wire_name(name: &str) -> Option<Self> {
        match name {
            "createSurface" | "CreateSurfaceMessage" => Some(Self::CreateSurface),
            "updateComponents" | "UpdateComponentsMessage" => Some(Self::UpdateComponents),
            "updateDataModel" | "UpdateDataModelMessage" => Some(Self::UpdateDataModel),
            "deleteSurface" | "DeleteSurfaceMessage" => Some(Self::DeleteSurface),
            "callRendererFunction" | "CallRendererFunctionMessage" => {
                Some(Self::CallRendererFunction)
            }
            _ => None,
        }
    }
}

/// What a prompt says beyond the catalogs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptOptions {
    /// How the model frames its block.
    pub framing: Framing,
    /// The messages the model may produce, or `None` for every one. The
    /// prompt teaches only the statements that compile to them.
    pub allowed: Option<Vec<AllowedMessage>>,
    /// Example turns, each the messages of one turn, shown in Express in
    /// the order given.
    pub examples: Vec<Vec<Value>>,
}

/// Which statements a prompt teaches: those that compile to an allowed
/// message.
#[derive(Debug, Clone, Copy)]
struct Teach<'o> {
    allowed: Option<&'o [AllowedMessage]>,
}

impl Teach<'_> {
    fn allows(self, message: AllowedMessage) -> bool {
        self.allowed
            .is_none_or(|allowed| allowed.contains(&message))
    }

    fn creates(self) -> bool {
        self.allows(AllowedMessage::CreateSurface)
    }

    fn components(self) -> bool {
        self.creates() || self.allows(AllowedMessage::UpdateComponents)
    }

    fn data(self) -> bool {
        self.allows(AllowedMessage::UpdateDataModel)
    }

    fn delete(self) -> bool {
        self.allows(AllowedMessage::DeleteSurface)
    }

    fn calls(self) -> bool {
        self.allows(AllowedMessage::CallRendererFunction)
    }
}

impl Express {
    /// The prompt that teaches a model to write Express for these
    /// catalogs.
    ///
    /// It states the syntax for the statements `options` allows, each
    /// component and function of each catalog as a positional signature,
    /// the protocol's system functions, each catalog's own instructions
    /// with their JSON examples written in Express, and the examples of
    /// `options`. A property many components share is described once.
    ///
    /// # Errors
    ///
    /// Fails when an example of `options` does not decompile, as
    /// [`Express::decompile`] fails.
    pub fn prompt(&self, options: &PromptOptions) -> Result<String, ExpressError> {
        let teach = Teach {
            allowed: options.allowed.as_deref(),
        };
        let callable = if teach.calls() {
            self.agent_callable_functions()
        } else {
            Vec::new()
        };
        let mut sections = vec![rules(options.framing, teach, &callable)];
        if self.is_multi_catalog() && (teach.components() || teach.data()) {
            sections.push(self.multi_catalog_rules());
        }
        if teach.components() {
            sections.push(self.system_functions());
            for (index, catalog) in self.catalogs.iter().enumerate() {
                let mut section = String::new();
                if self.is_multi_catalog() {
                    let _ = writeln!(section, "# Catalog `{}`\n", catalog.id());
                }
                section.push_str(&describe_catalog(&self.signatures[index]));
                if let Some(instructions) = catalog.instructions() {
                    let instructions = self.express_examples(instructions, options.framing);
                    let _ = write!(section, "\n\n## Catalog instructions\n\n{instructions}");
                }
                sections.push(section);
            }
        } else if !callable.is_empty() {
            sections.push(self.callable_signatures(&callable));
        }
        if !options.examples.is_empty() {
            let blocks = options
                .examples
                .iter()
                .map(|turn| Ok(frame(&self.decompile(turn)?, options.framing)))
                .collect::<Result<Vec<_>, ExpressError>>()?;
            sections.push(format!("## Examples\n\n{}", blocks.join("\n\n")));
        }
        Ok(sections.join("\n\n"))
    }

    fn multi_catalog_rules(&self) -> String {
        let ids: Vec<String> = self
            .catalogs
            .iter()
            .map(|c| format!("`{}`", c.id()))
            .collect();
        let mut text = format!(
            "## Several catalogs\n\nThese catalogs are active: {}. A surface has no catalog of its \
             own, so a `surface` line names none. Each component, function and check is found in \
             the catalog that defines its name. When several catalogs define a name, say which one \
             you mean with `catalogId=\"...\"` on the component or call, or `{{catalogId: \"...\"}}` \
             as an argument of a check.",
            ids.join(", ")
        );
        let mut shared: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
        for (index, signatures) in self.signatures.iter().enumerate() {
            let id = self.catalogs[index].id();
            for name in signatures.components.keys() {
                shared.entry(("Component", name)).or_default().push(id);
            }
            for name in signatures.functions.keys() {
                shared.entry(("Function", name)).or_default().push(id);
            }
        }
        let lines: Vec<String> = shared
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|((kind, name), ids)| format!("- {kind} `{name}`: {}", ids.join(", ")))
            .collect();
        if !lines.is_empty() {
            let _ = write!(text, " These names need a catalogId:\n{}", lines.join("\n"));
        }
        text
    }

    fn system_functions(&self) -> String {
        let lines: Vec<String> = self
            .system
            .iter()
            .map(|(name, sig)| function_line(name, sig, Call::Plain))
            .collect();
        format!(
            "## System functions\n\nThe protocol defines these functions, which no catalog does. \
             Call them by their `@` name, never with a catalogId.\n{}",
            lines.join("\n")
        )
    }

    fn callable_signatures(&self, callable: &[&str]) -> String {
        let lines: Vec<String> = callable
            .iter()
            .filter_map(|name| {
                let sig = self
                    .signatures
                    .iter()
                    .find_map(|s| s.functions.get(*name))?;
                Some(function_line(name, sig, Call::Plain))
            })
            .collect();
        format!("## Functions\n\n{}", lines.join("\n"))
    }

    /// `text` with each fenced JSON example of A2UI messages written in
    /// Express instead. One that does not decompile is left as it is.
    fn express_examples(&self, text: &str, framing: Framing) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("```json") {
            let body_start = start + "```json".len();
            let Some(body_len) = rest[body_start..].find("\n```") else {
                break;
            };
            let body = rest[body_start..body_start + body_len].trim();
            let end = body_start + body_len + "\n```".len();
            out.push_str(&rest[..start]);
            match self.decompile_example(body) {
                Some(express) => out.push_str(&fence(&express, framing)),
                None => out.push_str(&rest[start..end]),
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
        out
    }

    fn decompile_example(&self, json: &str) -> Option<String> {
        let parsed: Value = serde_json::from_str(json).ok()?;
        let messages = match parsed {
            Value::Array(messages) if !messages.is_empty() => messages,
            object @ Value::Object(_) => vec![object],
            _ => return None,
        };
        self.decompile(&messages).ok()
    }
}

/// Shows a block as the model would write it.
fn frame(block: &str, framing: Framing) -> String {
    match framing {
        Framing::Tagged => format!("<a2ui>\n{block}\n</a2ui>"),
        Framing::Bare => format!("```\n{block}\n```"),
    }
}

/// Shows a block inside prose, in a code fence.
fn fence(block: &str, framing: Framing) -> String {
    match framing {
        Framing::Tagged => format!("```\n{}\n```", frame(block, framing)),
        Framing::Bare => frame(block, framing),
    }
}

/// The syntax, for the statements `teach` allows.
fn rules(framing: Framing, teach: Teach<'_>, callable: &[&str]) -> String {
    let mut text = String::from(
        "# A2UI Express\n\nWrite the user interface in A2UI Express, a compact notation the host \
         compiles into A2UI messages.",
    );
    if framing == Framing::Tagged {
        text.push_str(
            " Put the whole Express block between the literal tags `<a2ui>` and `</a2ui>`, each on \
             a line of its own. Do not put the tags in backticks or a code fence.",
        );
    }
    let mut rules: Vec<&str> = Vec::new();
    if teach.components() {
        rules.push(
            "`name = Component(arguments)` defines a component, and the variable name becomes its \
             id. A component can also be written inline where an argument takes one, as in \
             `card = Parent(Child(\"Hi\"))`. Variable names are letters, digits and underscores.",
        );
    }
    if teach.creates() {
        rules.push("Assign the root component of a new surface to `root`.");
    }
    if teach.components() {
        rules.push(
            "Arguments follow the order of the signatures below. One ending in `?` is optional: \
             leave out trailing ones, write `_` to skip one, or pass any argument by name, as in \
             `name=value`.",
        );
    }
    let values = teach.components() || teach.data() || !callable.is_empty();
    if values {
        rules.push(
            "Values: strings in double quotes, with `\\n`, `\\t`, `\\\\` and `\\\"` escapes; \
         `\"\"\"...\"\"\"` for text across lines; `r\"...\"` for text with no escapes, such as a \
         regular expression. Numbers, `true`, `false` and `null`. Lists `[a, b]`. Maps \
         `{key: value}`, whose keys are literal.",
        );
        rules.push(
        "`$/path` binds an argument to a value in the data model. In a template item, `$name` is \
         relative to the item and `$` is the item itself. An argument marked `static` takes no \
         binding.",
        );
    }
    if teach.components() {
        rules.push(
            "An argument marked `component` takes one component. One marked `components` takes a \
             list of them, or `_template($/list, item)` to render the component `item` once for \
             each entry of the list at `$/list`.",
        );
        rules.push(
            "An action is either a function call, which the renderer runs, or \
             `Event(\"name\", {key: value}, userMessage=\"...\")`, which sends the agent an event. \
             The context map is optional, and so is `userMessage=`, the text shown for the user: a \
             string, a `$path` or a function call.",
        );
        rules.push(
            "A check, `?name` or `?name(arguments, \"message\")`, validates the component's bound \
             value and shows the message when it fails. Give several in a list. A check whose \
             component binds no value names it first: `?name($/path)`.",
        );
        rules.push("Call functions by name, with the same argument rules, where a value goes.");
    }
    if teach.data() {
        rules.push(
            "`$/path = value` writes the data model. In a block that assigns `root` it sets the new \
             surface's initial data; otherwise it updates that path of an existing surface.",
        );
    }
    let surface_rule;
    if teach.components() || teach.data() {
        surface_rule = if teach.creates() {
            "`surface(\"id\")` starts the statements for surface `id`; without it they target the \
             default surface. A block that assigns `root` creates the surface; without `root` it \
             updates a surface that exists, and `surface(\"id\", update=true)` updates one, `root` \
             included."
        } else {
            "`surface(\"id\")` starts the statements for the existing surface `id`; without it they \
             target the default surface."
        };
        rules.push(surface_rule);
    }
    if teach.delete() {
        rules.push("`deleteSurface(\"id\")` removes a surface.");
    }
    let call_rule;
    if let Some(first) = callable.first() {
        let names: Vec<String> = callable.iter().map(|name| format!("`{name}`")).collect();
        call_rule = format!(
            "A standalone call, assigned to nothing, asks the renderer to run a function right \
             away:\n{first}(...)\nOnly these functions can be called this way: {}.",
            names.join(", ")
        );
        rules.push(&call_rule);
    }
    let _ = write!(text, "\n\n## Syntax\n");
    for (number, rule) in rules.iter().enumerate() {
        let _ = write!(text, "\n{}. {rule}", number + 1);
    }
    text
}

/// A catalog's components and functions.
fn describe_catalog(signatures: &Signatures) -> String {
    let shared = shared_params(signatures);
    let mut sections = Vec::new();
    if !shared.is_empty() {
        let mut text = String::from(
            "## Shared arguments\n\nThese optional arguments are described once here. Pass them by name.",
        );
        for param in &shared {
            let holders: Vec<&str> = signatures
                .component_order
                .iter()
                .filter(|name| signatures.components[*name].params.contains(&param.name))
                .map(String::as_str)
                .collect();
            let scope = if holders.len() == signatures.components.len() {
                "every component".to_owned()
            } else {
                holders.join(", ")
            };
            let _ = write!(
                text,
                "\n- `{}=` ({scope}{}): {}",
                param.name, param.mark, param.description
            );
        }
        sections.push(text);
    }
    if !signatures.component_order.is_empty() {
        let mut text = String::from(
            "## Components\n\nWrite each as a call with these positional arguments.\n",
        );
        for name in &signatures.component_order {
            text.push_str(&component_entry(
                name,
                &signatures.components[name],
                &shared,
            ));
        }
        sections.push(text);
    }
    let (checks, functions): (Vec<&String>, Vec<&String>) = signatures
        .function_order
        .iter()
        .partition(|name| signatures.functions[*name].returns_check());
    if !functions.is_empty() {
        let lines: Vec<String> = functions
            .iter()
            .map(|name| function_line(name, &signatures.functions[*name], Call::Plain))
            .collect();
        sections.push(format!("## Functions\n\n{}", lines.join("\n")));
    }
    if !checks.is_empty() {
        let lines: Vec<String> = checks
            .iter()
            .map(|name| function_line(name, &signatures.functions[*name], Call::Check))
            .collect();
        sections.push(format!(
            "## Checks\n\nWrite these with `?`. Each checks the component's bound value, which it \
             takes as `value` unless you pass one first.\n\n{}",
            lines.join("\n")
        ));
    }
    sections.join("\n\n")
}

/// A property many components share with one schema, described once.
struct SharedParam {
    name: String,
    /// `, static` and the like, after the components that take it.
    mark: String,
    description: String,
}

/// The optional properties at least two components share with the same
/// schema and a description, which a prompt describes once.
fn shared_params(signatures: &Signatures) -> Vec<SharedParam> {
    let mut seen: BTreeMap<&str, (Option<&Value>, usize, bool)> = BTreeMap::new();
    for sig in signatures.components.values() {
        for param in &sig.params {
            let schema = sig.schema(param);
            let optional = !sig.required.contains(param);
            let entry = seen.entry(param).or_insert((schema, 0, true));
            entry.1 += 1;
            entry.2 &= entry.0 == schema && optional;
        }
    }
    seen.into_iter()
        .filter(|(_, (_, count, same))| *count > 1 && *same)
        .filter_map(|(name, (schema, _, _))| {
            let description = schema?.get("description")?.as_str()?;
            let mark = annotation(schema?)
                .map(|a| format!(", {}", a.trim_matches(['(', ')'])))
                .unwrap_or_default();
            Some(SharedParam {
                name: name.to_owned(),
                mark,
                description: one_line(description),
            })
        })
        .collect()
}

/// A component's signature line and the lines describing it.
fn component_entry(name: &str, sig: &ComponentSignature, shared: &[SharedParam]) -> String {
    let is_shared = |param: &str| shared.iter().any(|s| s.name == param);
    // A shared argument after every argument of the component's own is left
    // out of the signature: it is passed by name.
    let own_end = sig
        .params
        .iter()
        .rposition(|p| !is_shared(p) && p != "checks")
        .map_or(0, |i| i + 1);
    let mut args = Vec::new();
    for (index, param) in sig.params.iter().enumerate() {
        if index >= own_end && is_shared(param) {
            continue;
        }
        let optional = if sig.required.contains(param) {
            ""
        } else {
            "?"
        };
        let mark = sig
            .schema(param)
            .and_then(annotation)
            .map(|a| format!(" {a}"))
            .unwrap_or_default();
        args.push(format!("{param}{optional}{mark}"));
    }
    let mut text = format!("\n- {name}({})", args.join(", "));
    if let Some(description) = &sig.description {
        let _ = write!(text, ": {}", one_line(description));
    }
    for param in &sig.params {
        if is_shared(param) {
            continue;
        }
        let Some(schema) = sig.schema(param) else {
            continue;
        };
        if let Some(line) = param_line(param, schema) {
            let _ = write!(text, "\n  - {line}");
        }
    }
    text
}

/// The mark a signature puts on a parameter: what kind of value it takes,
/// when its name and enum do not say.
fn annotation(schema: &Value) -> Option<&'static str> {
    match signature::slot_kind(schema) {
        Some(SlotKind::Child) => Some("(component)"),
        Some(SlotKind::ChildList) => Some("(components)"),
        None if !signature::allows_binding(schema) && signature::find_enum(schema).is_none() => {
            Some("(static)")
        }
        Some(SlotKind::Action) | None => None,
    }
}

/// A parameter's description, enum and map keys, or `None` when it has
/// nothing to say.
fn param_line(name: &str, schema: &Value) -> Option<String> {
    let reference = signature::slot_kind(schema).is_some_and(|k| k != SlotKind::Action);
    let mut parts = Vec::new();
    if let Some(values) = signature::find_enum(schema) {
        let values: Vec<String> = values.iter().map(enum_value).collect();
        parts.push(format!("one of {}.", values.join(", ")));
    }
    if let Some(description) = schema.get("description").and_then(Value::as_str) {
        let description = if reference {
            express_reference_text(description)
        } else {
            one_line(description)
        };
        if !description.is_empty() {
            parts.push(description);
        }
    }
    let keys = schema
        .get("properties")
        .or_else(|| {
            schema
                .get("items")
                .and_then(|items| items.get("properties"))
        })
        .and_then(Value::as_object);
    if let Some(keys) = keys {
        let list = schema.get("items").is_some();
        let described: Vec<String> = keys
            .iter()
            .map(|(key, sub)| {
                let sub_reference =
                    signature::slot_kind(sub).is_some_and(|k| k != SlotKind::Action);
                let text = sub.get("description").and_then(Value::as_str).map(|d| {
                    if sub_reference {
                        express_reference_text(d)
                    } else {
                        one_line(d)
                    }
                });
                let mark = annotation(sub)
                    .filter(|a| *a != "(static)")
                    .map(|a| format!(" {a}"))
                    .unwrap_or_default();
                match text.filter(|t| !t.is_empty()) {
                    Some(text) => format!("\n    - {key}{mark}: {text}"),
                    None => format!("\n    - {key}{mark}"),
                }
            })
            .collect();
        let shape = if list {
            "Each item is a map with keys:"
        } else {
            "A map with keys:"
        };
        parts.push(format!("{shape}{}", described.concat()));
    }
    (!parts.is_empty()).then(|| format!("{name}: {}", parts.join(" ")))
}

fn enum_value(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => other.to_string(),
    }
}

/// A catalog's description of a child slot, which it writes for the JSON
/// wire form, where children are ids, rewritten for Express, where a child
/// is a component: sentences about ids, strings and template objects that
/// no rewording fixes are dropped.
fn express_reference_text(description: &str) -> String {
    const WIRE_ONLY: [&str; 6] = [
        "ID",
        "inline",
        "array of strings",
        "template object",
        "referred to",
        "IDs",
    ];
    let sentences: Vec<String> = sentences(description)
        .into_iter()
        .map(|sentence| {
            sentence
                .replace("The ID of the ", "The ")
                .replace("the ID of the ", "the ")
                .replace("'s ID here", " here")
                .replace("'s ID", "")
        })
        .filter(|sentence| !WIRE_ONLY.iter().any(|word| contains_word(sentence, word)))
        .collect();
    sentences.join(" ")
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Splits prose into sentences at `. ` and the like.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if matches!(c, '.' | '!' | '?') && chars.peek().is_none_or(|n| n.is_whitespace()) {
            out.push(current.trim().to_owned());
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

/// Text on one line, so it reads as one entry of a list.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How a function is written in its signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Plain,
    /// As a check, `?name(...)`, its leading `value` implied.
    Check,
}

fn function_line(name: &str, sig: &FunctionSignature, call: Call) -> String {
    let implied_value = call == Call::Check && sig.params.first().is_some_and(|p| p == "value");
    let params = if implied_value {
        &sig.params[1..]
    } else {
        &sig.params[..]
    };
    let args: Vec<String> = params
        .iter()
        .map(|p| {
            if sig.required.contains(p) {
                p.clone()
            } else {
                format!("{p}?")
            }
        })
        .collect();
    let mut text = match (call, args.is_empty()) {
        (Call::Check, true) => format!("- ?{name}"),
        (Call::Check, false) => format!("- ?{name}({})", args.join(", ")),
        (Call::Plain, _) => format!("- {name}({})", args.join(", ")),
    };
    if let Some(description) = &sig.description {
        let _ = write!(text, ": {}", indent_continuation(description));
    }
    for param in params {
        if let Some(description) = sig
            .schema(param)
            .and_then(|s| s.get("description"))
            .and_then(Value::as_str)
        {
            let _ = write!(text, "\n  - {param}: {}", indent_continuation(description));
        }
    }
    text
}

/// A description that may hold its own line breaks, such as a table of
/// date tokens, indented under its list entry.
fn indent_continuation(text: &str) -> String {
    let mut lines = text.trim().lines();
    let mut out = lines.next().unwrap_or_default().to_owned();
    for line in lines {
        out.push('\n');
        if !line.trim().is_empty() {
            out.push_str("    ");
            out.push_str(line.trim_end());
        }
    }
    out
}

impl FunctionSignature {
    /// Whether the function returns a check's outcome.
    fn returns_check(&self) -> bool {
        self.return_type == ReturnType::ValidationResult
    }
}
