use serde_json::{Map, Value};

use crate::catalog::Callers;
use crate::catalogs::Catalogs;
use crate::coerce::{number_value, to_number};
use crate::data_model::DataModel;
use crate::expr::{Call, Expr, ExpressionError};
use crate::functions::{Args, Effect, Function, Functions};
use crate::pointer::{resolve_path, Pointer};
use crate::template::MAX_DEPTH;

/// The deepest evaluation may recurse, counting bindings, containers and
/// calls, including those a `formatString` template evaluates. It leaves room
/// for any expression within [`MAX_DEPTH`] inside a template, and stops a
/// template that reads itself back from the data model.
const MAX_EVALUATION_DEPTH: usize = 2 * MAX_DEPTH;

/// Whether a user's gesture, such as a click, set off an evaluation. Only
/// then may a function that requires user activation, such as `openUrl`, run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum UserActivation {
    /// A binding, a check, or anything else the renderer evaluates by itself.
    #[default]
    Absent,
    /// An action the user triggered.
    Present,
}

/// Evaluates expressions against a data model, in one scope.
///
/// Relative bindings resolve against the scope path, and `@index` reads the
/// index of the collection item being evaluated.
#[derive(Debug)]
pub struct DataContext<'a> {
    model: &'a DataModel,
    calls: Calls<'a>,
    scope: Option<String>,
    index: Option<usize>,
    depth: usize,
    activation: UserActivation,
    /// The catalog of the function running now, which a template call that
    /// names no catalog inherits.
    running: Option<&'a str>,
    effects: Vec<Effect>,
}

/// Where a call finds its function.
#[derive(Debug, Clone, Copy)]
enum Calls<'a> {
    /// One table, whatever catalog the call names.
    Table(&'a Functions),
    /// The catalogs of a surface, resolved for each call.
    Surface {
        catalogs: &'a Catalogs,
        surface: &'a str,
        default: Option<&'a str>,
    },
}

impl<'a> DataContext<'a> {
    /// A context at the root of `model`, calling `functions` whatever catalog
    /// a call names.
    #[must_use]
    pub fn new(model: &'a DataModel, functions: &'a Functions) -> Self {
        Self::with_calls(model, Calls::Table(functions))
    }

    /// A context at the root of a surface's model, where each call runs in
    /// the catalog it names, else in the surface's default catalog.
    pub(crate) fn on_surface(
        model: &'a DataModel,
        catalogs: &'a Catalogs,
        surface: &'a str,
        default: Option<&'a str>,
    ) -> Self {
        Self::with_calls(
            model,
            Calls::Surface {
                catalogs,
                surface,
                default,
            },
        )
    }

    fn with_calls(model: &'a DataModel, calls: Calls<'a>) -> Self {
        Self {
            model,
            calls,
            scope: None,
            index: None,
            depth: 0,
            activation: UserActivation::Absent,
            running: None,
            effects: Vec::new(),
        }
    }

    /// Moves the scope to `path`, itself resolved against the current scope.
    #[must_use]
    pub fn with_scope(mut self, path: &str) -> Self {
        self.scope = Some(resolve_path(path, self.scope.as_deref()));
        self
    }

    /// Marks the scope as item `index` of a collection, which `@index` reads.
    #[must_use]
    pub fn with_index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    /// Says whether a user's gesture set off this evaluation.
    #[must_use]
    pub fn with_activation(mut self, activation: UserActivation) -> Self {
        self.activation = activation;
        self
    }

    /// Evaluates `expr`. A binding to a path with nothing there is null.
    ///
    /// # Errors
    ///
    /// Fails on an invalid path, an unknown function, `@index` outside a
    /// collection, a function that refuses its arguments, or recursion
    /// deeper than the evaluation bound. On a surface, a call also fails when
    /// its catalog cannot be resolved, when only the agent may call it, and
    /// when it requires user activation that is absent.
    pub fn evaluate(&mut self, expr: &Expr) -> Result<Value, ExpressionError> {
        match expr {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Path(path) => self.read(path),
            nested => {
                if self.depth >= MAX_EVALUATION_DEPTH {
                    return Err(ExpressionError::EvaluationTooDeep);
                }
                self.depth += 1;
                let value = self.evaluate_nested(nested);
                self.depth -= 1;
                value
            }
        }
    }

    /// Reads the value at `path`, resolved against the scope, or null.
    ///
    /// # Errors
    ///
    /// Fails when the resolved path is not a valid pointer.
    pub fn read(&self, path: &str) -> Result<Value, ExpressionError> {
        let pointer = self.pointer(path)?;
        Ok(self.model.get(&pointer).cloned().unwrap_or(Value::Null))
    }

    /// The absolute pointer `path` names, resolved against the scope.
    ///
    /// # Errors
    ///
    /// Fails when the resolved path is not a valid pointer.
    pub fn pointer(&self, path: &str) -> Result<Pointer, ExpressionError> {
        Ok(Pointer::parse(&resolve_path(path, self.scope.as_deref()))?)
    }

    /// Hands over the effects that functions asked for, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Asks the host to do something once the evaluation is over. A function
    /// calls this, since evaluation itself only reads.
    pub fn push_effect(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    fn evaluate_nested(&mut self, expr: &Expr) -> Result<Value, ExpressionError> {
        match expr {
            Expr::Literal(_) | Expr::Path(_) => self.evaluate(expr),
            Expr::List(items) => items
                .iter()
                .map(|item| self.evaluate(item))
                .collect::<Result<_, _>>()
                .map(Value::Array),
            Expr::Object(fields) => fields
                .iter()
                .map(|(key, field)| Ok((key.clone(), self.evaluate(field)?)))
                .collect::<Result<Map<_, _>, _>>()
                .map(Value::Object),
            Expr::Call(call) => self.call(call),
        }
    }

    fn call(&mut self, call: &Call) -> Result<Value, ExpressionError> {
        if call.name == "@index" {
            return self.index(call);
        }
        if call.name.starts_with('@') {
            return Err(ExpressionError::UnknownSystemFunction {
                name: call.name.clone(),
            });
        }
        let (catalog, function) = self.find(call)?;
        let args = self.evaluate_args(call)?;
        let outer = std::mem::replace(&mut self.running, catalog);
        let value = function(&args, self);
        self.running = outer;
        value.map_err(|source| ExpressionError::Function {
            name: call.name.clone(),
            source,
        })
    }

    /// The function a call runs and the catalog it runs in, checked against
    /// what that catalog declares about it.
    fn find(&self, call: &Call) -> Result<(Option<&'a str>, Function), ExpressionError> {
        let unknown = || ExpressionError::UnknownFunction {
            name: call.name.clone(),
        };
        match self.calls {
            Calls::Table(functions) => Ok((None, functions.get(&call.name).ok_or_else(unknown)?)),
            Calls::Surface {
                catalogs,
                surface,
                default,
            } => {
                let named = call.catalog_id.as_deref();
                // A template call names no catalog: it runs in the catalog of
                // the formatString that evaluates it.
                let (id, supported) = match named {
                    Some(named) => catalogs.resolve(Some(named), None, surface),
                    None => catalogs.resolve(self.running, default, surface),
                }?;
                if let Some(def) = supported.catalog.function(&call.name) {
                    if def.callers() == Callers::AgentOnly {
                        return Err(ExpressionError::AgentOnly {
                            name: call.name.clone(),
                        });
                    }
                    if def.requires_user_activation() && self.activation == UserActivation::Absent {
                        return Err(ExpressionError::UserActivationRequired {
                            name: call.name.clone(),
                        });
                    }
                }
                let function = supported.functions.get(&call.name).ok_or_else(unknown)?;
                Ok((Some(id), function))
            }
        }
    }

    fn evaluate_args(&mut self, call: &Call) -> Result<Args, ExpressionError> {
        call.args
            .iter()
            .map(|(name, arg)| Ok((name.clone(), self.evaluate(arg)?)))
            .collect::<Result<Map<_, _>, _>>()
            .map(Args::new)
    }

    /// `@index(offset)`: the collection index plus `offset`, 0 by default.
    fn index(&mut self, call: &Call) -> Result<Value, ExpressionError> {
        let index = self.index.ok_or(ExpressionError::IndexOutsideCollection)?;
        let offset = self
            .evaluate_args(call)?
            .get("offset")
            .map_or(0.0, to_number);
        let index = u32::try_from(index).map_or(f64::INFINITY, f64::from);
        Ok(number_value(index + offset))
    }
}

#[cfg(test)]
#[path = "data_context_tests.rs"]
mod tests;
