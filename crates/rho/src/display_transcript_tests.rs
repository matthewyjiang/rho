use pretty_assertions::assert_eq;

use super::*;

// Covers: future field renames must not silently invalidate saved v1 cards.
// Owner: on-disk display compatibility, independent of the current encoder.
#[test]
fn saved_v1_card_decodes_without_losing_metadata() {
    use rho_protocol::display::{
        NotificationDelivery, NotificationPreview, NotificationTone, NotificationVisibility,
    };
    let card = NotificationCard {
        title: "Update · Inspect replayability".into(),
        sender: "worker".into(),
        recipient: "parent".into(),
        delivery: NotificationDelivery::Received,
        tone: NotificationTone::Accent,
        preview: NotificationPreview::Truncated,
        visibility: NotificationVisibility::Conversation,
        reference: Some("abc123".into()),
        subtitle: None,
        body: "Schema inspected.\nReady for the next step.".into(),
        output: None,
        details: vec![
            "task: Inspect replayability".into(),
            "attach: rho attach abc123".into(),
        ],
    };
    let expected = DisplayTranscript(vec![DisplayRow::Notification(Box::new(NotificationRow {
        card,
        info: None,
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

// Covers: a typed card round-trips with its id, urgency, subject and items,
// and its saved form still reads as the plain card an older rho decodes.
// A miss mints a new id on every load, or leaves an older rho unable to
// show a newer session's cards.
// Owner: on-disk display compatibility.
#[test]
fn a_typed_card_round_trips_and_reads_as_a_plain_card() {
    use rho_protocol::display::{
        NotificationDelivery, NotificationItem, NotificationOutcome, NotificationPreview,
        NotificationSubject, NotificationTone, NotificationVisibility,
    };
    let card = NotificationCard {
        title: "Failed · Fix the build".into(),
        sender: "worker".into(),
        recipient: "parent".into(),
        delivery: NotificationDelivery::Received,
        tone: NotificationTone::Error,
        preview: NotificationPreview::Full,
        visibility: NotificationVisibility::Conversation,
        reference: Some("abc123".into()),
        subtitle: None,
        body: "cargo failed".into(),
        output: None,
        details: vec!["task: Fix the build".into()],
    };
    let subject = NotificationSubject::Subagent {
        run: "abc123".into(),
        session: None,
    };
    let row = NotificationRow::typed(
        card.clone(),
        subject.clone(),
        vec![NotificationItem::new(
            NotificationOutcome::Failed,
            rho_protocol::display::NotificationUrgency::NeedsYou,
            NotificationTone::Error,
            subject,
            "Failed · Fix the build",
        )],
    );
    let transcript = DisplayTranscript(vec![DisplayRow::Notification(Box::new(row))]);
    let Message::System(encoded) = transcript.display_message() else {
        panic!("display codec must not produce a human message");
    };

    let (_, json) = encoded.split_once('\n').expect("an envelope line");
    let saved: serde_json::Value = serde_json::from_str(json).unwrap();
    let older: NotificationCard = serde_json::from_value(saved[0]["Message"].clone()).unwrap();
    assert_eq!(
        (DisplayTranscript::from_display(&encoded), older),
        (Some(transcript), card)
    );
}
