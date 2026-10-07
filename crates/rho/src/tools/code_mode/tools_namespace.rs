//! The `tools` global: `tools.read_file({"path": p})` or
//! `tools.read_file(path=p)` runs one nested call, matching the
//! `tools.<name>(args)` shape models learn from Codex code mode.

use std::collections::BTreeMap;
use std::fmt;

use allocative::Allocative;
use serde_json::{Map, Value as JsonValue};
use starlark::environment::GlobalsBuilder;
use starlark::eval::{Arguments, Evaluator};
use starlark::starlark_simple_value;
use starlark::values::{starlark_value, NoSerialize, ProvidesStaticType, StarlarkValue, Value};

use super::engine::{call_one, starlark_to_json};

/// Build the catalog snapshot once. Exact names override aliases; ambiguous
/// aliases bind a diagnostic rather than silently selecting a tool.
pub(super) fn tools_namespace(builder: &mut GlobalsBuilder, names: Vec<String>) {
    let mut aliases = BTreeMap::<String, Vec<String>>::new();
    for name in &names {
        aliases
            .entry(identifier(name))
            .or_default()
            .push(name.clone());
    }
    let mut bindings: BTreeMap<_, _> = aliases
        .into_iter()
        .map(|(alias, mut candidates)| {
            let function = if candidates.len() == 1 {
                ToolFunction::Named {
                    name: candidates.pop().expect("one candidate"),
                }
            } else {
                ToolFunction::Ambiguous {
                    alias: alias.clone(),
                    names: candidates,
                }
            };
            (alias, function)
        })
        .collect();
    for name in names {
        bindings.insert(name.clone(), ToolFunction::Named { name });
    }
    builder.namespace("tools", |builder| {
        for (name, function) in bindings {
            builder.set(&name, function);
        }
    });
}

fn identifier(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// One bound tool. Arguments are an optional positional dict plus keyword
/// arguments; keywords override dict keys of the same name.
#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative)]
enum ToolFunction {
    Named { name: String },
    Ambiguous { alias: String, names: Vec<String> },
}

starlark_simple_value!(ToolFunction);

impl fmt::Display for ToolFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Named { name } => name,
            Self::Ambiguous { alias, .. } => alias,
        };
        write!(f, "tools.{name}")
    }
}

#[starlark_value(type = "function")]
impl<'v> StarlarkValue<'v> for ToolFunction {
    fn invoke(
        &self,
        _me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let name = match self {
            Self::Named { name } => name,
            Self::Ambiguous { alias, names } => {
                return Err(starlark::Error::new_other(anyhow::anyhow!(
                    "tools.{alias} is ambiguous between {}; use call_tool with an exact name",
                    names.join(", ")
                )));
            }
        };
        let invalid = || {
            starlark::Error::new_other(anyhow::anyhow!(
                "tools.{name} takes one dict of arguments and/or keyword arguments"
            ))
        };
        let positional: Vec<_> = args.positions(eval.heap())?.collect();
        let mut arguments = match positional.as_slice() {
            [] => Map::new(),
            [value] if value.is_none() => Map::new(),
            [value] => match starlark_to_json(*value)? {
                JsonValue::Object(map) => map,
                _ => return Err(invalid()),
            },
            _ => return Err(invalid()),
        };
        for (key, value) in args.names_map()? {
            arguments.insert(key.as_str().to_owned(), starlark_to_json(value)?);
        }
        call_one(name, JsonValue::Object(arguments), eval).map_err(starlark::Error::from)
    }
}

#[cfg(test)]
#[path = "tools_namespace_tests.rs"]
mod tests;
