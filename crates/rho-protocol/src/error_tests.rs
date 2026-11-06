use pretty_assertions::assert_eq;

use super::*;

/// Its position in [`ALL`]. The exhaustive match fails to compile when a
/// variant is added, which is the reminder to list it there.
fn listed_at(code: ErrorCode) -> usize {
    match code {
        ErrorCode::ParseError => 0,
        ErrorCode::InvalidRequest => 1,
        ErrorCode::MethodNotFound => 2,
        ErrorCode::InvalidParams => 3,
        ErrorCode::InternalError => 4,
        ErrorCode::HandshakeRequired => 5,
        ErrorCode::VersionMismatch => 6,
        ErrorCode::ThreadNotFound => 7,
        ErrorCode::ThreadNotLoaded => 8,
        ErrorCode::ThreadBusy => 9,
        ErrorCode::ThreadNotRunning => 10,
        ErrorCode::SteerRefused => 11,
        ErrorCode::DaemonStopping => 12,
        ErrorCode::ThreadOpenFailed => 13,
        ErrorCode::TurnFailed => 14,
        ErrorCode::TurnCancelled => 15,
    }
}

// Covers: every variant is in the table `from_code` reads, once, so each code
// a daemon sends decodes back to the variant it came from.
// Owner: protocol error codes.
#[test]
fn every_error_code_round_trips_through_its_wire_integer() {
    for (index, code) in ALL.into_iter().enumerate() {
        assert_eq!(listed_at(code), index, "{code:?}");
        assert_eq!(ErrorCode::from_code(code.code()), Some(code), "{code:?}");
    }
    assert_eq!(ErrorCode::from_code(-32099), None);
}
