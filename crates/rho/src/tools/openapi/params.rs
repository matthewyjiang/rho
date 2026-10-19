//! Operation parameters: merging path item and operation declarations, and
//! rejecting serializations tools cannot send.

use serde_json::{Map, Value};

use super::{
    document::{Document, JsonPointer},
    operation::SkipReason,
};

/// OpenAPI ignores header parameters with these names; transport sets them.
const IGNORED_HEADER_PARAMS: &[&str] = &["accept", "content-type", "authorization"];

/// Parameters grouped by where they are sent.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Params {
    pub(crate) path: Vec<Param>,
    pub(crate) query: Vec<Param>,
    pub(crate) header: Vec<Param>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Param {
    pub(crate) name: String,
    /// Always true for path parameters.
    pub(crate) required: bool,
    pub(crate) style: ParamStyle,
    pub(crate) explode: bool,
    pub(crate) schema: JsonPointer,
}

/// The parameter serializations tools support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamStyle {
    /// Path and header parameters.
    Simple,
    /// Query parameters, the default.
    Form,
    /// Query objects as `name[key]=value`.
    DeepObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParamLocation {
    Path,
    Query,
    Header,
}

/// One declaration, after `$ref`s are followed.
struct Declared<'a> {
    name: String,
    location: String,
    at: JsonPointer,
    param: &'a Map<String, Value>,
}

/// Parse the parameters of an operation. `owners` are the path item and then
/// the operation, each with its location: operation parameters override path
/// item parameters with the same name and location.
pub(super) fn parse(
    document: &Document,
    owners: [(&JsonPointer, &Map<String, Value>); 2],
    path: &str,
) -> Result<Params, SkipReason> {
    let mut declared: Vec<Declared<'_>> = Vec::new();
    for (owner_at, owner) in owners {
        let Some(Value::Array(list)) = owner.get("parameters") else {
            continue;
        };
        let list_at = owner_at.child("parameters");
        for index in 0..list.len() {
            let (at, param) = document.resolve_at(list_at.index(index))?;
            let Value::Object(param) = param else {
                return Err(SkipReason::InvalidParam("not an object"));
            };
            let (Some(name), Some(location)) = (
                param.get("name").and_then(Value::as_str),
                param.get("in").and_then(Value::as_str),
            ) else {
                return Err(SkipReason::InvalidParam("missing name or location"));
            };
            let entry = Declared {
                name: name.to_owned(),
                location: location.to_owned(),
                at,
                param,
            };
            match declared
                .iter_mut()
                .find(|seen| seen.name == name && seen.location == location)
            {
                Some(seen) => *seen = entry,
                None => declared.push(entry),
            }
        }
    }

    let mut params = Params::default();
    for Declared {
        name,
        location,
        at,
        param,
    } in declared
    {
        let location = match location.as_str() {
            "path" => ParamLocation::Path,
            "query" => ParamLocation::Query,
            "header" if IGNORED_HEADER_PARAMS.contains(&name.to_ascii_lowercase().as_str()) => {
                continue;
            }
            "header" => ParamLocation::Header,
            "cookie" => return Err(SkipReason::CookieParam { name }),
            _ => return Err(SkipReason::UnsupportedParamLocation { name, location }),
        };
        let unsupported = |style: &str| SkipReason::UnsupportedParamStyle {
            name: name.clone(),
            style: style.to_owned(),
        };
        let malformed = |problem| SkipReason::MalformedParam {
            name: name.clone(),
            problem,
        };
        if param.contains_key("content") {
            return Err(unsupported("content"));
        }
        if !param.contains_key("schema") {
            return Err(malformed("has neither schema nor content"));
        }
        if param.get("allowReserved") == Some(&Value::Bool(true)) {
            return Err(unsupported("allowReserved"));
        }
        let declared_style = match param.get("style") {
            None => None,
            Some(Value::String(style)) => Some(style.as_str()),
            Some(_) => return Err(malformed("has a style that is not a string")),
        };
        let style = match (location, declared_style) {
            (ParamLocation::Path | ParamLocation::Header, None | Some("simple")) => {
                ParamStyle::Simple
            }
            (ParamLocation::Query, None | Some("form")) => ParamStyle::Form,
            (ParamLocation::Query, Some("deepObject")) => ParamStyle::DeepObject,
            (_, Some(style)) => return Err(unsupported(style)),
        };
        let explode = match param.get("explode") {
            None => style == ParamStyle::Form,
            Some(Value::Bool(explode)) => *explode,
            Some(_) => return Err(malformed("has an explode that is not a boolean")),
        };
        if style == ParamStyle::DeepObject && !explode {
            return Err(unsupported("deepObject without explode"));
        }
        let required = param.get("required") == Some(&Value::Bool(true));
        let group = match location {
            ParamLocation::Path => &mut params.path,
            ParamLocation::Query => &mut params.query,
            ParamLocation::Header => &mut params.header,
        };
        group.push(Param {
            required: required || location == ParamLocation::Path,
            style,
            explode,
            schema: at.child("schema"),
            name,
        });
    }

    for name in path_template_params(path) {
        if !params.path.iter().any(|param| param.name == name) {
            return Err(SkipReason::UndeclaredPathParam {
                name: name.to_owned(),
            });
        }
    }
    Ok(params)
}

/// Names of `{param}` segments in a path template.
fn path_template_params(path: &str) -> impl Iterator<Item = &str> {
    path.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
}
