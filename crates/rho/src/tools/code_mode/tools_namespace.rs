//! The `tools` global: `tools.read_file({"path": p})` or
//! `tools.read_file(path=p)` runs one nested call, matching the
//! `tools.<name>(args)` shape models learn from Codex code mode.

use std::fmt;

use allocative::Allocative;
use serde_json::{Map, Value as JsonValue};
use starlark::eval::{Arguments, Evaluator};
use starlark::starlark_simple_value;
use starlark::values::{
    starlark_value, Heap, NoSerialize, ProvidesStaticType, StarlarkValue, Value,
};

use super::engine::{call_one, starlark_to_json};

/// Attribute names are identifiers, so tool names like `get-docs` are
/// reachable as `tools.get_docs`. Exact names win over normalized ones.
#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative)]
pub(super) struct ToolsNamespace {
    names: Vec<String>,
}

starlark_simple_value!(ToolsNamespace);

impl ToolsNamespace {
    pub(super) fn new(names: Vec<String>) -> Self {
        Self { names }
    }

    fn resolve(&self, attribute: &str) -> Option<&str> {
        let exact = self.names.iter().find(|name| *name == attribute);
        exact
            .or_else(|| self.names.iter().find(|name| identifier(name) == attribute))
            .map(String::as_str)
    }
}

fn identifier(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

impl fmt::Display for ToolsNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tools")
    }
}

#[starlark_value(type = "tools")]
impl<'v> StarlarkValue<'v> for ToolsNamespace {
    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        let name = self.resolve(attribute)?.to_owned();
        Some(heap.alloc(ToolFunction { name }))
    }

    fn has_attr(&self, attribute: &str, _heap: Heap<'v>) -> bool {
        self.resolve(attribute).is_some()
    }

    fn dir_attr(&self) -> Vec<String> {
        self.names.iter().map(|name| identifier(name)).collect()
    }
}

/// One bound tool. Arguments are an optional positional dict plus keyword
/// arguments; keywords override dict keys of the same name.
#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative)]
struct ToolFunction {
    name: String,
}

starlark_simple_value!(ToolFunction);

impl fmt::Display for ToolFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tools.{}", self.name)
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
        let invalid = || {
            starlark::Error::new_other(anyhow::anyhow!(
                "tools.{} takes one dict of arguments and/or keyword arguments",
                self.name
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
        call_one(&self.name, JsonValue::Object(arguments), eval).map_err(starlark::Error::from)
    }
}
