//! Last bytes of a redirected CLI log file, for `result.json` error context.

use std::path::Path;

/// Bytes of log file kept on a failed CLI run.
///
/// 400 bytes is enough for the last error line of a CLI's stderr (observed
/// Cursor and Claude startup errors are 60–300 bytes) while keeping
/// `result.json` small.
const LOG_TAIL_BYTES: usize = 400;

/// Read the tail of `path`, or empty when the file cannot be read.
pub(crate) async fn read_log_tail(path: &Path) -> String {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return String::new();
    };
    let trimmed = contents.trim();
    if trimmed.len() <= LOG_TAIL_BYTES {
        return trimmed.to_string();
    }
    let cut = trimmed.len() - LOG_TAIL_BYTES;
    let boundary = rho_sdk::ceil_char_boundary(trimmed, cut);
    format!("{}{}", rho_sdk::ELLIPSIS, &trimmed[boundary..])
}
