use serde_json::json;

use super::*;
use crate::functions::FunctionError;

fn evaluate(
    model: &DataModel,
    scope: Option<(&str, usize)>,
    template: &str,
) -> Result<Value, ExpressionError> {
    let functions = Functions::basic();
    let mut context = DataContext::new(model, &functions);
    if let Some((path, index)) = scope {
        context = context.with_scope(path).with_index(index);
    }
    let call = Expr::from_json(&json!({"@call": "formatString", "args": {"value": template}}))?;
    context.evaluate(&call)
}

/// Relative bindings read the scope's item and `@index` its position;
/// outside a collection `@index` is an error rather than a made-up 0.
#[test]
fn scope_and_index() {
    let model = DataModel::new(json!({"items": [{"name": "a"}, {"name": "b"}]}));
    assert_eq!(
        evaluate(
            &model,
            Some(("/items/1", 1)),
            "${@index(offset: 1)}. ${name}"
        ),
        Ok(json!("2. b"))
    );
    assert!(matches!(
        evaluate(&model, None, "${@index()}"),
        Err(ExpressionError::Function { source: FunctionError::Template(e), .. })
            if *e == ExpressionError::IndexOutsideCollection
    ));
}

/// A template that reads itself back from the data model fails instead
/// of recursing until the stack overflows.
#[test]
fn self_referencing_template_stops() {
    let model = DataModel::new(json!({"t": "${formatString(value: /t)}"}));
    let error =
        evaluate(&model, None, "${formatString(value: /t)}").expect_err("recursion is bounded");
    let mut source: &ExpressionError = &error;
    while let ExpressionError::Function {
        source: FunctionError::Template(inner),
        ..
    } = source
    {
        source = inner;
    }
    assert_eq!(*source, ExpressionError::EvaluationTooDeep);
}
