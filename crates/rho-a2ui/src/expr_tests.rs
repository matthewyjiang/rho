use serde_json::json;

use super::*;

/// Wire forms parse into the shape evaluation relies on: bindings and
/// calls found at any depth, literal-only containers kept whole, `@@`
/// keys unescaped, and malformed directives refused.
#[test]
fn wire_forms() {
    let call = Expr::from_json(&json!({
        "@call": "and",
        "catalogId": "basic",
        "args": {"values": [true, {"@path": "/ok"}]},
    }));
    assert_eq!(
        call,
        Ok(Expr::Call(Call {
            name: "and".to_owned(),
            args: BTreeMap::from([(
                "values".to_owned(),
                Expr::List(vec![
                    Expr::Literal(json!(true)),
                    Expr::Path("/ok".to_owned()),
                ]),
            )]),
            catalog_id: Some("basic".to_owned()),
        }))
    );

    let cases = [
        (
            json!({"a": [1, {"b": 2}]}),
            Ok(Expr::Literal(json!({"a": [1, {"b": 2}]}))),
        ),
        (
            json!({"@@path": "/x"}),
            Ok(Expr::Literal(json!({"@path": "/x"}))),
        ),
        (
            json!({"@type": 1}),
            Err(ExpressionError::UnknownDirective {
                key: "@type".to_owned(),
            }),
        ),
        (
            json!({"@": 1}),
            Err(ExpressionError::UnknownDirective {
                key: "@".to_owned(),
            }),
        ),
        (
            json!({"@path": "/x", "extra": 1}),
            Err(ExpressionError::MalformedBinding),
        ),
        (json!({"@path": 3}), Err(ExpressionError::MalformedBinding)),
        (
            json!({"@call": "f", "returnType": "any"}),
            Err(ExpressionError::MalformedCall),
        ),
        (
            json!({"@call": "f", "args": [1]}),
            Err(ExpressionError::MalformedCall),
        ),
    ];
    for (wire, want) in cases {
        assert_eq!(Expr::from_json(&wire), want, "{wire}");
    }
}
