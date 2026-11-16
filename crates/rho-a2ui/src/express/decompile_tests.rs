use serde_json::Number;

use super::*;

/// Doubles at the upper bound of plain decimal, where ECMAScript
/// switches to exponent notation, and negative ones, which the
/// decompile suite does not reach.
#[test]
fn numbers_follow_ecmascript_at_the_edges() {
    let cases = [
        (1e20, "100000000000000000000.0"),
        (1e21, "1e+21"),
        (-0.0, "-0.0"),
        (-2.5e-7, "-2.5e-7"),
    ];
    for (value, want) in cases {
        let number = Number::from_f64(value).expect("finite");
        assert_eq!(number_literal(&number).as_deref(), Ok(want), "{value:e}");
    }
}
