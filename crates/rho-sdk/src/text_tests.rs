use pretty_assertions::assert_eq;

use super::{ceil_char_boundary, floor_char_boundary};

// Covers: a mid-codepoint cut must land on a real character boundary.
// Owner: pure unit (sdk text helpers)
#[test]
fn char_boundary_helpers_align_cuts_to_codepoint_edges() {
    let value = "aébc";

    assert_eq!(
        [
            floor_char_boundary(value, 0),
            floor_char_boundary(value, 1),
            floor_char_boundary(value, 2),
            floor_char_boundary(value, 3),
            floor_char_boundary(value, 99),
        ],
        [0, 1, 1, 3, value.len()]
    );
    assert_eq!(
        [
            ceil_char_boundary(value, 0),
            ceil_char_boundary(value, 1),
            ceil_char_boundary(value, 2),
            ceil_char_boundary(value, 3),
            ceil_char_boundary(value, 99),
        ],
        [0, 1, 3, 3, value.len()]
    );
    assert!(value.is_char_boundary(floor_char_boundary(value, 2)));
    assert!(value.is_char_boundary(ceil_char_boundary(value, 2)));
}
