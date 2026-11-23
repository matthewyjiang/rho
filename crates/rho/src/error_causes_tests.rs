use pretty_assertions::assert_eq;

use super::with_causes;

/// A wrapper whose `Display` leaves its source out, as `reqwest`'s does.
#[derive(Debug, thiserror::Error)]
#[error("error sending request")]
struct Hides(#[source] std::io::Error);

/// A wrapper whose `Display` shows its source, as rmcp's `Io` does.
#[derive(Debug, thiserror::Error)]
#[error("Io error: {0}")]
struct Shows(#[source] std::io::Error);

// Covers: a cause the text leaves out is appended; one it already shows
// is not repeated.
// Owner: error text with causes (unit).
#[test]
fn causes_are_appended_once() {
    let refused = || std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
    assert_eq!(
        (
            with_causes(&Hides(refused())),
            with_causes(&Shows(refused()))
        ),
        (
            "error sending request: refused".to_owned(),
            "Io error: refused".to_owned()
        )
    );
}
