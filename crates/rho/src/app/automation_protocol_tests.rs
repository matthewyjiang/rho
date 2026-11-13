use std::time::Duration;

use pretty_assertions::assert_eq;

use super::{parse_duration, JsonlStream, RunEnd};

#[test]
fn parses_human_durations_and_rejects_invalid_values() {
    assert_eq!(parse_duration("20m").unwrap(), Duration::from_secs(1_200));
    assert!(parse_duration("soon")
        .unwrap_err()
        .contains("invalid duration"));
}

/// A writer that fails its writes from the `fails_at`th on, keeping what it
/// was given before.
struct Failing {
    written: Vec<u8>,
    writes: usize,
    fails_at: usize,
}

impl std::io::Write for Failing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if self.writes >= self.fails_at {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.written.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Covers: lines are numbered from 1 under `schema_version` 2; a completed
// run's `run_ended` says nothing more; the first failed write is the
// stream's failure and nothing is written after it, not even the end.
// Owner: rho run JSONL framing.
#[test]
fn the_stream_numbers_its_lines_and_stops_at_its_first_failure() {
    let lines = |stream: &JsonlStream<Failing>| -> Vec<serde_json::Value> {
        String::from_utf8(stream.out.written.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let mut stream = JsonlStream::new(Failing {
        written: Vec::new(),
        writes: 0,
        fails_at: usize::MAX,
    });
    stream.run_started("session-1", std::path::Path::new("/workspace"));
    stream.event(&rho_protocol::SessionEvent::Notice {
        text: "hello".into(),
    });
    stream.end(&RunEnd::completed());
    assert_eq!(
        lines(&stream),
        [
            serde_json::json!({"schema_version": 2, "seq": 1, "type": "run_started", "session": "session-1", "workspace": "/workspace"}),
            serde_json::json!({"schema_version": 2, "seq": 2, "type": "event", "event": {"type": "notice", "text": "hello"}}),
            serde_json::json!({"schema_version": 2, "seq": 3, "type": "run_ended", "reason": "completed", "exit_code": 0}),
        ]
    );
    assert_eq!(stream.failure(), None);

    let mut broken = JsonlStream::new(Failing {
        written: Vec::new(),
        writes: 0,
        fails_at: 1,
    });
    broken.run_started("session-1", std::path::Path::new("/workspace"));
    broken.end(&RunEnd::completed());
    assert_eq!(broken.out.writes, 1, "nothing is written after the failure");
    assert_eq!(
        broken.failure(),
        Some("could not write JSONL output: broken pipe")
    );
}
