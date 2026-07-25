//! Claude stream-json NDJSON line bound and re-export of the shared decoder.
//!
//! Decoding lives in `rho_providers::provider_backend::line_decoder`. This
//! module only owns the Claude-specific 4 MiB anti-runaway budget and a thin
//! constructor so session call sites stay self-documenting.

use rho_providers::provider_backend::line_decoder::LineDecoder as SharedLineDecoder;

/// Maximum accepted bytes in one NDJSON line, including an incomplete tail.
///
/// # Why 4 MiB (not 1 MiB, not 32 MiB)
///
/// Claude stream-json lines are usually small. With
/// `--include-partial-messages`, tool inputs arrive as short
/// `input_json_delta` chunks. Live fixtures stay under 2 KiB per line.
///
/// The complete `assistant` envelope and `user`/`tool_result` frames still
/// carry full tool input/result JSON on a single NDJSON line. Agents may
/// declare `Write` / `Read` / `Bash`, so a legitimate supported message can
/// hold hundreds of KiB to a few MiB of file or command output. A 1 MiB
/// decoder cap would reject those complete frames before the mapper can
/// apply its softer presentation bounds (tool payload display 16 KiB,
/// terminal result 64 KiB, text/reasoning delta 32 KiB).
///
/// 4 MiB is therefore the anti-runaway line budget: large enough for
/// ordinary multi-megabyte tool envelopes, far below a 16–32 MiB "accept
/// anything" ceiling. Peak retained memory is one incomplete line of this
/// size; oversize input sets a pending error, drops only that tail, and
/// the session fails the run with a clear diagnostic rather than truncating
/// mid-JSON and corrupting later lines.
pub(crate) const MAX_NDJSON_LINE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) use rho_providers::provider_backend::line_decoder::LineDecodeError;

/// Bounded line decoder for Claude local stream-json stdout.
pub(crate) type LineDecoder = SharedLineDecoder;

/// Build the Claude stream-json decoder with the 4 MiB anti-runaway budget.
pub(crate) fn claude_ndjson_line_decoder() -> LineDecoder {
    SharedLineDecoder::with_max_line_bytes(MAX_NDJSON_LINE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::{claude_ndjson_line_decoder, LineDecodeError, MAX_NDJSON_LINE_BYTES};

    #[test]
    fn claude_budget_rejects_oversize_and_accepts_near_1mib_tool_result() {
        // Evidence that the cap is about wire NDJSON size, not display size: a
        // complete tool_result frame near 1 MiB (large Read/Bash output) must
        // parse as a line under the 4 MiB budget.
        let payload_chars = 1024 * 1024 - 256;
        let envelope = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_big","content":"{}"}}]}}}}"#,
            "y".repeat(payload_chars)
        );
        assert!(
            envelope.len() > 1024 * 1024 - 512,
            "fixture should exercise multi-megabyte-class tool_result frames"
        );
        assert!(
            envelope.len() <= MAX_NDJSON_LINE_BYTES,
            "legitimate tool_result must fit the decoder budget"
        );

        let mut decoder = claude_ndjson_line_decoder();
        decoder.push(envelope.as_bytes());
        decoder.push(b"\n");
        let line = decoder
            .next_line()
            .expect("near-1MiB tool_result is legitimate")
            .expect("line present");
        assert_eq!(line, envelope);

        decoder.push(&vec![b'z'; MAX_NDJSON_LINE_BYTES + 1]);
        decoder.push(b"\n");
        let error = decoder
            .next_line()
            .expect_err("oversize after valid large line");
        assert!(matches!(
            error,
            LineDecodeError::LineTooLong {
                limit: MAX_NDJSON_LINE_BYTES,
                ..
            }
        ));
    }
}
