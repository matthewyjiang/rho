//! The envelope each nested call returns to a script. Models trained on
//! JavaScript tool APIs read `r.content` first, so the envelope answers both
//! `r.content` and `r["content"]`. It is deliberately not a dict proxy: `data`
//! and everything inside it stay native Starlark values, so dict operations on
//! tool data behave exactly as they do on any other dict.

use std::fmt;

use allocative::Allocative;
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value as JsonValue;
use starlark::coerce::Coerce;
use starlark::starlark_complex_value;
use starlark::values::{
    starlark_value, Freeze, Heap, ProvidesStaticType, StarlarkValue, Trace, Value, ValueError,
    ValueLifetimeless, ValueLike,
};

/// Field names, in display and serialization order.
const FIELDS: [&str; 3] = ["is_error", "content", "data"];

/// Allocates a `{is_error, content, data}` envelope from `script_output`.
pub(super) fn alloc_tool_result<'v>(heap: Heap<'v>, envelope: &JsonValue) -> Value<'v> {
    heap.alloc(ToolResult {
        is_error: heap.alloc(&envelope["is_error"]),
        content: heap.alloc(&envelope["content"]),
        data: heap.alloc(&envelope["data"]),
    })
}

#[derive(Debug, Trace, Freeze, Coerce, ProvidesStaticType, Allocative)]
#[repr(C)]
pub(super) struct ToolResultGen<V: ValueLifetimeless> {
    is_error: V,
    content: V,
    data: V,
}

starlark_complex_value!(pub(super) ToolResult);

impl<V: ValueLifetimeless> ToolResultGen<V> {
    fn fields(&self) -> [(&'static str, V); 3] {
        let [is_error, content, data] = FIELDS;
        [
            (is_error, self.is_error),
            (content, self.content),
            (data, self.data),
        ]
    }

    fn field(&self, name: &str) -> Option<V> {
        self.fields()
            .into_iter()
            .find_map(|(field, value)| (field == name).then_some(value))
    }
}

impl<V: ValueLifetimeless> fmt::Display for ToolResultGen<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tool_result(is_error={}, content={}, data={})",
            self.is_error, self.content, self.data
        )
    }
}

impl<V: ValueLifetimeless> Serialize for ToolResultGen<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(FIELDS.len()))?;
        for (name, value) in self.fields() {
            map.serialize_entry(name, &value)?;
        }
        map.end()
    }
}

#[starlark_value(type = "tool_result")]
impl<'v, V: ValueLike<'v>> StarlarkValue<'v> for ToolResultGen<V>
where
    Self: ProvidesStaticType<'v>,
{
    fn get_attr(&self, attribute: &str, _heap: Heap<'v>) -> Option<Value<'v>> {
        self.field(attribute).map(ValueLike::to_value)
    }

    fn has_attr(&self, attribute: &str, _heap: Heap<'v>) -> bool {
        self.field(attribute).is_some()
    }

    fn dir_attr(&self) -> Vec<String> {
        FIELDS.map(str::to_owned).to_vec()
    }

    fn at(&self, index: Value<'v>, _heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        index
            .unpack_str()
            .and_then(|key| self.field(key))
            .map(ValueLike::to_value)
            .ok_or_else(|| starlark::Error::new_other(ValueError::KeyNotFound(index.to_repr())))
    }

    fn is_in(&self, other: Value<'v>) -> starlark::Result<bool> {
        Ok(other
            .unpack_str()
            .is_some_and(|key| self.field(key).is_some()))
    }

    /// Reads like the dict envelope it replaced, so printed results stay familiar.
    fn collect_repr(&self, collector: &mut String) {
        collector.push('{');
        for (index, (name, value)) in self.fields().into_iter().enumerate() {
            if index > 0 {
                collector.push_str(", ");
            }
            collector.push_str(&format!("\"{name}\": "));
            value.to_value().collect_repr(collector);
        }
        collector.push('}');
    }

    /// Equal only to another envelope, which keeps `==` symmetric with dicts.
    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        let Some(other) = ToolResult::from_value(other) else {
            return Ok(false);
        };
        for ((_, mine), (_, theirs)) in self.fields().into_iter().zip(other.fields()) {
            if !mine.to_value().equals(theirs)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "tool_result_tests.rs"]
mod tests;
