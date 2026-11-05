//! NDJSON line bound for agent stdout and re-export of the shared decoder.
//!
//! Decoding lives in `rho_providers::provider_backend::line_decoder`. This
//! module only owns the 4 MiB anti-runaway budget.

/// Maximum accepted bytes in one NDJSON line, including an incomplete tail.
///
/// # Why 4 MiB (not 1 MiB, not 32 MiB)
///
/// Stream-json lines are usually small. With partial-message streaming,
/// tool inputs arrive as short `input_json_delta` chunks. Live fixtures
/// stay under 2 KiB per line.
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

/// Run-failure text for a stdout line the decoder refused.
pub(crate) fn format_line_error(error: &LineDecodeError, program_label: &str) -> String {
    match error {
        LineDecodeError::InvalidUtf8(_) => {
            format!("{program_label}: malformed UTF-8 on stdout: {error}")
        }
        LineDecodeError::LineTooLong { .. } => {
            format!("{program_label}: oversize stdout line: {error}")
        }
    }
}

#[cfg(test)]
#[path = "line_decoder_tests.rs"]
mod tests;
