use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

use super::*;

fn preview(key: &str, detail: &str) -> AttachmentEvent {
    AttachmentEvent::ToolPreview {
        key: key.into(),
        card: ToolCard::new(
            ToolStatus::Running,
            ToolFamily::Default,
            ToolHeader::call("write", Some(detail.into())),
        )
        .into(),
    }
}

fn durable(event: &AttachmentEvent) -> AttachmentEvent {
    let AttachmentEvent::ToolPreview { key, card } = event else {
        panic!("expected preview")
    };
    AttachmentEvent::ToolStarted {
        key: Some(key.clone()),
        card: card.clone(),
    }
}

// Covers: stale live files must not resurrect completed/reset tools, or hide an
// unrelated concurrent preview after an abrupt exit between file writes. A
// snapshot ahead of the journal must wait for the next complete journal read.
// Owner: attachment journal/snapshot ordering contract.
#[test]
fn stale_snapshots_obey_durable_key_and_reset_boundaries() {
    let a = preview("preview:0", "partial a");
    let b = preview("call-b", "partial b");
    for (boundary, remaining) in [
        (durable(&a), vec![b.clone()]),
        (
            AttachmentEvent::ToolFinished {
                key: Some("preview:0".into()),
                presentation: match durable(&a) {
                    AttachmentEvent::ToolStarted { card, .. } => card.into(),
                    _ => unreachable!(),
                },
            },
            vec![b.clone()],
        ),
        (AttachmentEvent::ProviderStreamReset, vec![]),
        (AttachmentEvent::Completed, vec![]),
        (AttachmentEvent::Cancelled, vec![]),
        (AttachmentEvent::Failed("interrupted".into()), vec![]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(subagent::ATTACHMENT_FILE_NAME);
        let snapshot_path = directory.path().join("tool-previews.json");
        let mut writer = AttachmentWriter::create(&directory.path().join("result.json")).unwrap();
        let prompt = AttachmentEvent::Prompt("task".into());
        writer.write_event(&prompt).unwrap();
        writer.write_event(&a).unwrap();
        writer.write_event(&b).unwrap();
        let snapshot = std::fs::read(&snapshot_path).unwrap();
        let journal = std::fs::read(&path).unwrap();
        // Simulate a snapshot published after the reader's journal read ended.
        std::fs::write(&path, "").unwrap();
        let mut live = AttachmentReader::new(path.clone());
        assert_eq!(live.read_new().unwrap(), vec![]);
        std::fs::write(&path, journal).unwrap();
        assert_eq!(live.read_new().unwrap(), vec![prompt, a.clone(), b.clone()]);
        assert_eq!(live.read_new().unwrap(), vec![]);
        writer.write_event(&boundary).unwrap();
        // Reproduce the state after a crash left a stale atomic snapshot.
        std::fs::write(&snapshot_path, snapshot).unwrap();
        drop(writer);
        let new_events = live.read_new().unwrap();
        assert!(new_events
            .iter()
            .all(|event| !matches!(event, AttachmentEvent::ToolPreview { .. })));
        assert_eq!(live.read_new().unwrap(), vec![]);
        let mut replay = AttachmentReader::new(path);
        let replay = replay.read_new().unwrap();
        assert_eq!(
            replay
                .into_iter()
                .filter(|event| matches!(event, AttachmentEvent::ToolPreview { .. }))
                .collect::<Vec<_>>(),
            remaining
        );
    }
}

// Covers: input streamed after a Claude start must survive finish or orderly
// interruption once, even without a complete assistant envelope.
// Owner: attachment lifecycle persistence.
#[test]
fn latest_uncommitted_input_is_saved_at_lifecycle_boundaries() {
    let first = preview("call", "first");
    let latest = preview("call", "complete input");
    let AttachmentEvent::ToolPreview { key, card } = &latest else {
        unreachable!()
    };
    let finished = AttachmentEvent::ToolFinished {
        key: Some(key.clone()),
        presentation: card.clone().into(),
    };
    for (terminal, mut expected) in [
        (Some(AttachmentEvent::Cancelled), vec![durable(&latest)]),
        (None, vec![durable(&latest)]),
        (
            Some(finished),
            vec![AttachmentEvent::ToolUpdated {
                key: Some(key.clone()),
                card: card.clone(),
            }],
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let result = directory.path().join("result.json");
        let mut writer = AttachmentWriter::create(&result).unwrap();
        writer.write_event(&first).unwrap();
        writer.write_event(&latest).unwrap();
        if let Some(event) = &terminal {
            writer.write_event(event).unwrap();
        } else {
            writer.flush_previews().unwrap();
        }
        drop(writer);
        expected.extend(terminal);
        let mut reader =
            AttachmentReader::new(result.with_file_name(subagent::ATTACHMENT_FILE_NAME));
        assert_eq!(reader.read_new().unwrap(), expected);
    }
}

// Covers: a broken optional preview transport must not consume or disable valid
// durable history. Owner: run-artifact transport failure isolation.
#[test]
fn preview_transport_failures_leave_history_readable_and_writable() {
    for invalid_file in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let result = directory.path().join("result.json");
        let sidecar = directory.path().join("tool-previews.json");
        let mut writer = AttachmentWriter::create(&result).unwrap();
        let prompt = AttachmentEvent::Prompt("task".into());
        writer.write_event(&prompt).unwrap();
        if invalid_file {
            std::fs::write(&sidecar, "not json").unwrap();
        } else {
            std::fs::create_dir(&sidecar).unwrap();
        }
        let mut reader =
            AttachmentReader::new(result.with_file_name(subagent::ATTACHMENT_FILE_NAME));
        let events = reader.read_new().unwrap();
        assert_eq!(events.first(), Some(&prompt));
        assert!(matches!(events.as_slice(), [_, AttachmentEvent::Notice(_)]));
        assert_eq!(reader.read_new().unwrap(), vec![]);
        let notice = AttachmentEvent::Notice("durable event".into());
        writer.write_event(&notice).unwrap();
        assert_eq!(reader.read_new().unwrap(), vec![notice]);

        if !invalid_file {
            // Replacing a directory fails even for a privileged test process,
            // while the already-open journal remains appendable.
            writer.write_event(&preview("call", "first")).unwrap();
            let latest = preview("call", "latest input");
            writer.write_event(&latest).unwrap();
            writer.write_event(&AttachmentEvent::Cancelled).unwrap();
            std::fs::remove_dir(sidecar).unwrap();
            let events = reader.read_new().unwrap();
            assert!(matches!(events.first(), Some(AttachmentEvent::Notice(_))));
            assert_eq!(
                &events[1..],
                &[durable(&latest), AttachmentEvent::Cancelled]
            );
        }
    }
}
