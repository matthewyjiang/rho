use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::catalogs::UnresolvedCatalog;
use crate::functions::FunctionError;
use crate::pointer::{check_escapes, PointerError};
use crate::template::{MAX_DEPTH, MAX_TEMPLATE_CHARS, MAX_TEMPLATE_PARTS};

/// A dynamic value, parsed: a literal, a data binding or a function call.
///
/// Both the wire's JSON forms ([`Expr::from_json`]) and the `${...}` template
/// language ([`parse_template`](crate::parse_template)) parse into it, so no
/// later stage looks for `@path` or `@call` in raw JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A value with no binding or call inside it. Object keys are unescaped:
    /// a wire key `@@path` is the literal key `@path` here.
    Literal(Value),
    /// A data binding to a JSON Pointer path as written: absolute, or
    /// relative to the scope it is evaluated in.
    Path(String),
    /// A function call.
    Call(Call),
    /// A list with at least one binding or call inside it.
    List(Vec<Expr>),
    /// An object with at least one binding or call inside it, keys unescaped.
    Object(BTreeMap<String, Expr>),
}

/// A function call: `{"@call": name, "args": {...}}` on the wire, or
/// `name(arg: value)` in a template.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The function's name. System functions such as `@index` keep their `@`.
    pub name: String,
    /// The named arguments.
    pub args: BTreeMap<String, Expr>,
    /// The catalog the call names, if any. A template call never names one.
    pub catalog_id: Option<String>,
}

/// Why an expression failed to parse or evaluate.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ExpressionError {
    /// A `${` with no matching `}`.
    #[error("unclosed interpolation: missing '}}'")]
    UnclosedInterpolation,
    /// A quoted string with no closing quote.
    #[error("unclosed string literal")]
    UnclosedString,
    /// Text left over after a complete expression.
    #[error("unexpected characters at end of expression: {rest:?}")]
    UnexpectedCharacters {
        /// The text left over.
        rest: String,
    },
    /// A call argument whose name is not followed by `:`. A positional
    /// argument, which the language does not have, fails this way.
    #[error("expected ':' after argument name {argument:?} in a call to {function}")]
    MissingArgumentColon {
        /// The function being called.
        function: String,
        /// The argument name read so far, possibly empty.
        argument: String,
    },
    /// A call whose argument list does not end with `)`.
    #[error("expected ')' after function arguments for {function}")]
    UnclosedCall {
        /// The function being called.
        function: String,
    },
    /// A number literal outside the grammar, such as `1.2.3` or `1e`.
    #[error("invalid number literal {literal:?}")]
    InvalidNumber {
        /// The literal as written.
        literal: String,
    },
    /// A number literal beyond the range of a double, such as `1e999`.
    #[error("number literal {literal:?} is out of range")]
    NumberOutOfRange {
        /// The literal as written.
        literal: String,
    },
    /// A `~` in a path that does not start `~0` or `~1`.
    #[error("invalid escape in path {path:?}: '~' must be followed by '0' or '1'")]
    InvalidPathEscape {
        /// The path read up to the bad escape.
        path: String,
    },
    /// A function name containing `~`.
    #[error("'~' is not allowed in function name {name:?}")]
    TildeInFunctionName {
        /// The name as written.
        name: String,
    },
    /// An `@` not followed by an ASCII letter or `_`.
    #[error("invalid identifier starting with '@'")]
    InvalidSystemName,
    /// A system name such as `@index` used without a call.
    #[error("expected '(' after system function name {name}")]
    SystemNameWithoutCall {
        /// The name as written.
        name: String,
    },
    /// Nesting of interpolations, call arguments or wire containers beyond
    /// [`MAX_DEPTH`].
    #[error("max recursion depth of {MAX_DEPTH} exceeded")]
    TooDeep,
    /// A template longer than [`MAX_TEMPLATE_CHARS`] characters.
    #[error("template of {chars} characters exceeds the limit of {MAX_TEMPLATE_CHARS}")]
    TemplateTooLong {
        /// The template's length in characters.
        chars: usize,
    },
    /// A template of more than [`MAX_TEMPLATE_PARTS`] parts.
    #[error("template exceeds the limit of {MAX_TEMPLATE_PARTS} parts")]
    TooManyParts,
    /// A wire object key that starts with a single `@` and is no directive.
    #[error("unknown directive {key:?}: escape a literal '@' key as '@@'")]
    UnknownDirective {
        /// The key as written.
        key: String,
    },
    /// An object with `@path` that is not exactly `{"@path": <string>}`.
    #[error("a data binding must be exactly {{\"@path\": <string>}}")]
    MalformedBinding,
    /// An object with `@call` whose name is not a string, whose `args` is not
    /// an object, whose `catalogId` is not a string, or with any other key.
    #[error("a function call must be {{\"@call\": <string>, \"args\"?: {{...}}, \"catalogId\"?: <string>}}")]
    MalformedCall,
    /// A binding whose path is not a valid pointer once resolved.
    #[error(transparent)]
    InvalidPath(#[from] PointerError),
    /// A call to a function the context does not have.
    #[error("unknown function {name:?}")]
    UnknownFunction {
        /// The name called.
        name: String,
    },
    /// A call to an `@` name other than the system functions the protocol
    /// defines, `@index`.
    #[error("unknown system function {name:?}")]
    UnknownSystemFunction {
        /// The name called.
        name: String,
    },
    /// A call on a surface whose catalog cannot be resolved.
    #[error(transparent)]
    UnresolvedCatalog(#[from] UnresolvedCatalog),
    /// A renderer call to a function whose catalog lets only the agent call
    /// it.
    #[error("function {name:?} is agentOnly and cannot be called by renderer")]
    AgentOnly {
        /// The function called.
        name: String,
    },
    /// A call, outside a user's gesture, to a function that requires one.
    #[error("function {name:?} requires user activation")]
    UserActivationRequired {
        /// The function called.
        name: String,
    },
    /// `@index` evaluated outside a collection item.
    #[error("@index function can only be evaluated inside a collection template iteration scope.")]
    IndexOutsideCollection,
    /// Evaluation, through templates that call functions, recursed too deep.
    #[error("evaluation exceeded the maximum recursion depth")]
    EvaluationTooDeep,
    /// A function refused its arguments.
    #[error("{name}: {source}")]
    Function {
        /// The function called.
        name: String,
        /// Why it refused.
        source: FunctionError,
    },
}

impl Expr {
    /// Parses a dynamic value in its wire form.
    ///
    /// `{"@path": p}` is a binding and `{"@call": name, "args": {...},
    /// "catalogId": id}` a call, whose arguments are parsed the same way.
    /// Other objects and lists are parsed member by member. A key starting
    /// `@@` loses one `@`, and any other key starting with a single `@` is
    /// rejected. Strings are literals: only `formatString` reads `${...}`.
    ///
    /// # Errors
    ///
    /// Fails on a malformed binding or call, an unknown `@` key, or nesting
    /// deeper than [`MAX_DEPTH`].
    pub fn from_json(value: &Value) -> Result<Self, ExpressionError> {
        parse_wire(value, 0)
    }
}

fn parse_wire(value: &Value, depth: usize) -> Result<Expr, ExpressionError> {
    if depth > MAX_DEPTH {
        return Err(ExpressionError::TooDeep);
    }
    match value {
        Value::Array(items) => {
            let items = items
                .iter()
                .map(|item| parse_wire(item, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            match items.iter().map(literal).collect::<Option<Vec<_>>>() {
                Some(values) => Ok(Expr::Literal(Value::Array(values))),
                None => Ok(Expr::List(items)),
            }
        }
        Value::Object(map) if map.contains_key("@path") => parse_binding(map),
        Value::Object(map) if map.contains_key("@call") => parse_call(map, depth),
        Value::Object(map) => {
            let mut fields = BTreeMap::new();
            for (key, member) in map {
                fields.insert(unescape_key(key)?, parse_wire(member, depth + 1)?);
            }
            let literals = fields
                .iter()
                .map(|(key, field)| Some((key.clone(), literal(field)?)))
                .collect::<Option<Map<_, _>>>();
            match literals {
                Some(members) => Ok(Expr::Literal(Value::Object(members))),
                None => Ok(Expr::Object(fields)),
            }
        }
        scalar => Ok(Expr::Literal(scalar.clone())),
    }
}

/// The value of a literal, or `None` for anything dynamic.
fn literal(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Literal(value) => Some(value.clone()),
        _ => None,
    }
}

/// `@@x` is the literal key `@x`. Any other single-`@` key is reserved.
fn unescape_key(key: &str) -> Result<String, ExpressionError> {
    match key.strip_prefix('@') {
        Some(rest) if rest.starts_with('@') => Ok(rest.to_owned()),
        Some(_) => Err(ExpressionError::UnknownDirective {
            key: key.to_owned(),
        }),
        None => Ok(key.to_owned()),
    }
}

fn parse_binding(map: &Map<String, Value>) -> Result<Expr, ExpressionError> {
    match map.get("@path") {
        Some(Value::String(path)) if map.len() == 1 => {
            check_escapes(path)
                .map_err(|_| ExpressionError::InvalidPathEscape { path: path.clone() })?;
            Ok(Expr::Path(path.clone()))
        }
        _ => Err(ExpressionError::MalformedBinding),
    }
}

fn parse_call(map: &Map<String, Value>, depth: usize) -> Result<Expr, ExpressionError> {
    if map
        .keys()
        .any(|key| !matches!(key.as_str(), "@call" | "args" | "catalogId"))
    {
        return Err(ExpressionError::MalformedCall);
    }
    let Some(Value::String(name)) = map.get("@call") else {
        return Err(ExpressionError::MalformedCall);
    };
    let catalog_id = match map.get("catalogId") {
        None => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => return Err(ExpressionError::MalformedCall),
    };
    let args = match map.get("args") {
        None => BTreeMap::new(),
        Some(Value::Object(args)) => args
            .iter()
            .map(|(arg, value)| Ok((arg.clone(), parse_wire(value, depth + 1)?)))
            .collect::<Result<_, ExpressionError>>()?,
        Some(_) => return Err(ExpressionError::MalformedCall),
    };
    Ok(Expr::Call(Call {
        name: name.clone(),
        args,
        catalog_id,
    }))
}

#[cfg(test)]
#[path = "expr_tests.rs"]
mod tests;
