use pretty_assertions::assert_eq;

use super::*;

// Covers: each code a daemon sends decodes back to the variant it came
// from, and a code this crate does not define decodes to none.
// Owner: protocol error codes.
#[test]
fn every_error_code_round_trips_through_its_wire_integer() {
    for &code in ALL {
        assert_eq!(ErrorCode::from_code(code.code()), Some(code), "{code:?}");
    }
    assert_eq!(ErrorCode::from_code(-32099), None);
}
