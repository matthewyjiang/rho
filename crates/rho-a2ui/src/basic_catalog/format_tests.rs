use super::*;

/// Rounding and grouping as `Intl.NumberFormat` does them for `en-US`.
#[test]
fn decimals_round_on_the_exact_value() {
    let cases = [
        (2.5, 0, 0, "3"),
        (-2.5, 0, 0, "-3"),
        (1.005, 2, 2, "1.00"),
        (9.995, 2, 2, "9.99"),
        (999.9996, 0, 3, "1,000"),
        (1234.5, 0, 3, "1,234.5"),
        (0.1 + 0.2, 0, 3, "0.3"),
        (1e21, 0, 0, "1,000,000,000,000,000,000,000"),
        (-0.0001, 0, 0, "-0"),
    ];
    for (number, min, max, want) in cases {
        assert_eq!(decimal(number, min, max, Grouping::On), want, "{number}");
    }
}
