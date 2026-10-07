//! JSON objects handed to scripts. Each one is a real Starlark dict that also
//! answers `obj.key` reads, because models trained on JavaScript tool APIs
//! write `result.content` before `result["content"]`.

use std::fmt;

use allocative::Allocative;
use serde::{Serialize, Serializer};
use serde_json::Value as JsonValue;
use starlark::coerce::Coerce;
use starlark::starlark_complex_value;
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::AllocList;
use starlark::values::{
    starlark_value, Freeze, Heap, ProvidesStaticType, StarlarkValue, Trace, Value,
    ValueLifetimeless, ValueLike,
};

/// Allocates `json` with every object, at any depth, as a [`JsonObject`].
pub(super) fn alloc_json<'v>(heap: Heap<'v>, json: &JsonValue) -> Value<'v> {
    match json {
        JsonValue::Object(map) => {
            let dict = heap.alloc(AllocDict(
                map.iter()
                    .map(|(key, value)| (key.as_str(), alloc_json(heap, value))),
            ));
            heap.alloc(JsonObject { dict })
        }
        JsonValue::Array(items) => {
            heap.alloc(AllocList(items.iter().map(|item| alloc_json(heap, item))))
        }
        scalar => heap.alloc(scalar),
    }
}

/// Wraps a dict. Keys win over dict methods for attribute reads, so data with
/// an `items` key reads as data; `obj["items"]` and `obj.get(...)` still work.
#[derive(Debug, Trace, Freeze, Coerce, ProvidesStaticType, Allocative)]
#[repr(C)]
pub(super) struct JsonObjectGen<V: ValueLifetimeless> {
    dict: V,
}

starlark_complex_value!(pub(super) JsonObject);

impl<V: ValueLifetimeless> fmt::Display for JsonObjectGen<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.dict, f)
    }
}

impl<V: ValueLifetimeless> Serialize for JsonObjectGen<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.dict.serialize(serializer)
    }
}

impl<'v, V: ValueLike<'v>> JsonObjectGen<V> {
    fn dict(&self) -> Value<'v> {
        self.dict.to_value()
    }

    fn field(&self, key: &str) -> Option<Value<'v>> {
        DictRef::from_value(self.dict())?.get_str(key)
    }
}

#[starlark_value(type = "dict")]
impl<'v, V: ValueLike<'v>> StarlarkValue<'v> for JsonObjectGen<V>
where
    Self: ProvidesStaticType<'v>,
{
    fn get_attr(&self, attribute: &str, heap: Heap<'v>) -> Option<Value<'v>> {
        self.field(attribute)
            .or_else(|| self.dict().get_attr(attribute, heap).ok().flatten())
    }

    fn has_attr(&self, attribute: &str, heap: Heap<'v>) -> bool {
        self.field(attribute).is_some() || self.dict().has_attr(attribute, heap)
    }

    fn dir_attr(&self) -> Vec<String> {
        let mut names = self.dict().dir_attr();
        if let Some(dict) = DictRef::from_value(self.dict()) {
            names.extend(
                dict.keys()
                    .filter_map(|key| key.unpack_str().map(str::to_owned)),
            );
        }
        names
    }

    fn at(&self, index: Value<'v>, heap: Heap<'v>) -> starlark::Result<Value<'v>> {
        self.dict().at(index, heap)
    }

    fn set_at(&self, index: Value<'v>, new_value: Value<'v>) -> starlark::Result<()> {
        self.dict().set_at(index, new_value)
    }

    fn iterate_collect(&self, heap: Heap<'v>) -> starlark::Result<Vec<Value<'v>>> {
        Ok(self.dict().iterate(heap)?.collect())
    }

    fn length(&self) -> starlark::Result<i32> {
        self.dict().length()
    }

    fn is_in(&self, other: Value<'v>) -> starlark::Result<bool> {
        self.dict().is_in(other)
    }

    fn to_bool(&self) -> bool {
        self.dict().to_bool()
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        let other = JsonObject::from_value(other).map_or(other, |object| object.dict);
        self.dict().equals(other)
    }
}

#[cfg(test)]
#[path = "json_object_tests.rs"]
mod tests;
