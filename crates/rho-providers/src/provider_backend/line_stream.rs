//! Shared HTTP byte-stream pump: idle deadline, line decode, per-line dispatch.
//!
//! Protocol event handling stays at the call site. `handle_line` returns whether
//! the line was meaningful activity for the idle deadline. The finish tail is
//! dispatched the same way as a complete line and does not reset the deadline.

use futures_util::StreamExt;

use super::{
    line_decoder::{LineDecodeError, LineDecoder},
    stream_timeout::StreamIdleDeadline,
    ModelError,
};

/// Reads `response` as UTF-8 lines and invokes `handle_line` for each.
pub(crate) async fn collect_line_stream(
    response: reqwest::Response,
    mut map_decode_error: impl FnMut(LineDecodeError) -> ModelError,
    mut handle_line: impl FnMut(&str) -> Result<bool, ModelError>,
) -> Result<(), ModelError> {
    let mut decoder = LineDecoder::default();
    let mut stream = response.bytes_stream();
    let mut idle_deadline = StreamIdleDeadline::new();
    loop {
        let Some(chunk) = idle_deadline.wait_for(stream.next()).await? else {
            break;
        };
        decoder.push(&chunk?);
        while let Some(line) = decoder.next_line().map_err(&mut map_decode_error)? {
            if handle_line(line)? {
                idle_deadline.record_activity();
            }
        }
    }
    if let Some(line) = decoder.finish().map_err(&mut map_decode_error)? {
        handle_line(line)?;
    }
    Ok(())
}
