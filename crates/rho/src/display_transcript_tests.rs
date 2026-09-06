use pretty_assertions::assert_eq;

use super::*;

// Covers: future field renames must not silently invalidate saved v1 cards.
// Owner: on-disk display compatibility, independent of the current encoder.
#[test]
fn saved_v1_card_decodes_without_losing_metadata() {
    use crate::presentation::{MessageDelivery, MessagePreview, MessageTone, MessageVisibility};
    let expected = DisplayTranscript(vec![DisplayRow::Message(Box::new(MessageCard {
        title: "Update · Inspect replayability".into(),
        sender: "worker".into(),
        recipient: "parent".into(),
        delivery: MessageDelivery::Received,
        tone: MessageTone::Accent,
        preview: MessagePreview::Truncated,
        visibility: MessageVisibility::Conversation,
        reference: Some("abc123".into()),
        body: "Schema inspected.\nReady for the next step.".into(),
        details: vec![
            "task: Inspect replayability".into(),
            "attach: rho attach abc123".into(),
        ],
    }))]);
    let fixture = include_str!("display_transcript_v1.fixture").replace("\r\n", "\n");
    for encoded in [fixture.clone(), fixture.replace('\n', "\r\n")] {
        assert_eq!(
            DisplayTranscript::from_display(&encoded),
            Some(expected.clone())
        );
    }
}

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
