use pretty_assertions::assert_eq;

use super::*;

// Covers: saved notifications round-trip, and unknown/damaged records do not
// silently vanish. Owner: shared host display codec, not any renderer.
#[test]
fn display_codec_preserves_records_or_reports_unsupported_data() {
    let transcript = DisplayTranscript(vec![DisplayRow::Notice("delivered result".into())]);
    let Message::System(encoded) = transcript.display_message() else {
        panic!("display codec must not produce a human message");
    };
    assert_eq!(DisplayTranscript::from_display(&encoded), Some(transcript));
    assert_eq!(
        DisplayTranscript::from_display("ordinary system text"),
        None
    );
    for encoded in [
        "[rho boundary transcript v1]\nnot json",
        "[rho boundary transcript v2]\n[]",
        "[rho boundary transcript v1]\n[{\"FutureRow\":{}}]",
    ] {
        let restored =
            DisplayTranscript::from_display(encoded).expect("recognized display envelope");
        assert!(matches!(restored.0.as_slice(), [DisplayRow::Notice(_)]));
    }
}
