use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::basic_catalog::{date, format, logic, url, validation};
use crate::coerce;
use crate::data_context::DataContext;
use crate::expr::ExpressionError;
use crate::pointer::Pointer;
use crate::regex_dialect::RegexError;

/// A function's implementation. It gets its arguments already evaluated and
/// the context of the call, which it may use to evaluate more.
pub type Function = fn(&Args, &mut DataContext<'_>) -> Result<Value, FunctionError>;

/// The basic catalog's functions, by name. Those that cannot fail are
/// wrapped to the common signature here.
const BASIC: [(&str, Function); 14] = [
    ("required", |args, _| Ok(validation::required(args))),
    ("regex", |args, _| validation::regex(args)),
    ("length", |args, _| Ok(validation::length(args))),
    ("numeric", |args, _| Ok(validation::numeric(args))),
    ("email", |args, _| Ok(validation::email(args))),
    ("formatString", format::format_string),
    ("formatNumber", |args, _| Ok(format::format_number(args))),
    ("formatCurrency", |args, _| {
        Ok(format::format_currency(args))
    }),
    ("formatDate", |args, _| Ok(date::format_date(args))),
    ("pluralize", |args, _| Ok(format::pluralize(args))),
    ("openUrl", url::open_url),
    ("and", |args, _| logic::and(args)),
    ("or", |args, _| logic::or(args)),
    ("not", |args, _| Ok(logic::not(args))),
];

/// The functions a [`DataContext`] can call, by name.
///
/// System functions such as `@index` are not here: the context answers them.
#[derive(Debug, Clone, Default)]
pub struct Functions {
    table: BTreeMap<String, Function>,
}

impl Functions {
    /// No functions, for a catalog whose functions run elsewhere or that
    /// declares none.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `function` as `name`, replacing any function of that name.
    pub fn insert(&mut self, name: &str, function: Function) {
        self.table.insert(name.to_owned(), function);
    }

    /// The functions of A2UI's basic catalog: the validators `required`,
    /// `regex`, `length`, `numeric` and `email`; `formatString`,
    /// `formatNumber`, `formatCurrency`, `formatDate` and `pluralize`, which
    /// format as `en-US` does; `openUrl`; and `and`, `or` and `not`.
    #[must_use]
    pub fn basic() -> Self {
        Self {
            table: BASIC
                .into_iter()
                .map(|(name, function)| (name.to_owned(), function))
                .collect(),
        }
    }

    /// The function called `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Function> {
        self.table.get(name).copied()
    }
}

/// A call's arguments, evaluated, with readers that apply the A2UI
/// coercions for the type an argument is declared with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args(Map<String, Value>);

impl Args {
    pub(crate) fn new(values: Map<String, Value>) -> Self {
        Self(values)
    }

    /// The argument called `name`, or `None` when the call has none.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// The argument as a `DynamicString`: absent or null is empty, other
    /// values are written as text.
    #[must_use]
    pub fn string(&self, name: &str) -> String {
        self.get(name).map(coerce::to_string).unwrap_or_default()
    }

    /// The argument as a `DynamicNumber`: absent, null or not numeric is 0.
    #[must_use]
    pub fn number(&self, name: &str) -> f64 {
        self.get(name).map_or(0.0, coerce::to_number)
    }

    /// The argument as a `DynamicBoolean`: absent or null is false, and a
    /// `ValidationResult` reads as its validity.
    #[must_use]
    pub fn boolean(&self, name: &str) -> bool {
        self.get(name).is_some_and(coerce::to_bool)
    }
}

/// Something a function asks the host to do, which evaluation cannot do
/// itself. [`DataContext::take_effects`] hands them over.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Open this URL, already checked to be `http`, `https`, `mailto` or `tel`.
    OpenUrl(String),
    /// Write `value` at `path` in the data model of the surface the call ran
    /// on, as an action's function call may.
    SetData {
        /// Where to write.
        path: Pointer,
        /// What to write; null removes, as `updateDataModel` does.
        value: Value,
    },
}

/// Why a function refused its arguments.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FunctionError {
    /// `values` is not a list.
    #[error("values must be a list")]
    ValuesNotAList,
    /// `values` has fewer than two entries.
    #[error("values needs at least 2 entries, got {got}")]
    TooFewValues {
        /// How many it has.
        got: usize,
    },
    /// The pattern is not in the A2UI regex dialect or fails its safety check.
    #[error(transparent)]
    Regex(#[from] RegexError),
    /// A URL with no scheme.
    #[error("URL must be absolute: {url:?}")]
    RelativeUrl {
        /// The URL as given.
        url: String,
    },
    /// A URL whose scheme is not `http`, `https`, `mailto` or `tel`.
    #[error("unsupported URL scheme {scheme:?}")]
    UnsupportedUrlScheme {
        /// The scheme as written.
        scheme: String,
    },
    /// A template that fails to parse, or a part of it that fails to
    /// evaluate.
    #[error("in template: {0}")]
    Template(#[from] Box<ExpressionError>),
    /// A function the host defines failed, for the reason given.
    #[error("{0}")]
    Failed(String),
}
