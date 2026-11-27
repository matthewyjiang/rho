use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;

use super::*;

const INFO: &[u8] = b"info";

// Covers: an issued key opens what was sealed to it exactly once; a failed
// open, a tampered `aad`, or a key past its lifetime opens nothing; and any
// attempt on a held key uses it up, so a second attempt is `Gone`.
// Owner: seal mechanics (`sync/seal.rs`).
#[test]
fn a_recipient_opens_one_bundle_once_within_its_lifetime() {
    let issued = Instant::now();
    let cases: [(&str, Duration, &[u8], Result<&[u8], OpenError>); 3] = [
        (
            "opens within the lifetime",
            RECIPIENT_LIFETIME - Duration::from_secs(1),
            b"aad",
            Ok(b"secret"),
        ),
        (
            "an altered aad does not open",
            Duration::ZERO,
            b"other aad",
            Err(OpenError::Invalid(
                "it was not sealed to this recipient, or was altered",
            )),
        ),
        (
            "an expired key is gone",
            RECIPIENT_LIFETIME,
            b"aad",
            Err(OpenError::Gone),
        ),
    ];
    for (name, after, aad, expected) in cases {
        let recipients = Recipients::default();
        let recipient = recipients.issue(issued);
        let sealed = seal(&recipient, INFO, b"aad", b"secret").unwrap();
        let opened = recipients.open(issued + after, &recipient, &sealed, INFO, aad);
        assert_eq!(
            opened.as_ref().map(|plain| plain.as_slice()),
            expected.as_ref().map(|plain| *plain),
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
