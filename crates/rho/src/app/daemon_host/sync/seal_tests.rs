use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;

use super::*;

const INFO: &[u8] = b"info";

struct Case {
    name: &'static str,
    /// When the open runs, after the key was issued.
    after: Duration,
    /// The `aad` the open names; the bundle was sealed with `b"aad"`.
    aad: &'static [u8],
    opened: Result<&'static [u8], OpenError>,
}

// Covers: an issued key opens what was sealed to it exactly once; a failed
// open, a tampered `aad`, or a key past its lifetime opens nothing; and any
// attempt on a held key uses it up, so a second attempt is `Gone`.
// Owner: seal mechanics (`sync/seal.rs`).
#[test]
fn a_recipient_opens_one_bundle_once_within_its_lifetime() {
    let issued = Instant::now();
    let cases = [
        Case {
            name: "opens within the lifetime",
            after: RECIPIENT_LIFETIME - Duration::from_secs(1),
            aad: b"aad",
            opened: Ok(b"secret"),
        },
        Case {
            name: "an altered aad does not open",
            after: Duration::ZERO,
            aad: b"other aad",
            opened: Err(OpenError::Invalid(InvalidBundle::Unauthentic)),
        },
        Case {
            name: "an expired key is gone",
            after: RECIPIENT_LIFETIME,
            aad: b"aad",
            opened: Err(OpenError::Gone),
        },
    ];
    for Case {
        name,
        after,
        aad,
        opened,
    } in cases
    {
        let recipients = Recipients::default();
        let recipient = recipients.issue(issued);
        let sealed = seal(&recipient, INFO, b"aad", b"secret").unwrap();
        assert_eq!(
            recipients
                .open(issued + after, &recipient, &sealed, INFO, aad)
                .map(|plain| plain.to_vec()),
            opened.map(<[u8]>::to_vec),
            "{name}"
        );
        assert_eq!(
            recipients
                .open(issued + after, &recipient, &sealed, INFO, b"aad")
                .err(),
            Some(OpenError::Gone),
            "{name}: a second attempt"
        );
    }
}
