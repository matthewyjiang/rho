//! Bounded capture of child process stderr for diagnostics.
//!
//! Reading stderr to EOF in a single buffer allows a chatty child process to
//! grow memory without bound. [`StderrTail`] keeps the closing tail of bytes
//! (which usually carries failure diagnostics) and truncates on clean UTF-8
//! character boundaries.

/// Default bytes of child stderr kept for diagnosis.
pub(crate) const MAX_STDERR_BYTES: usize = 8 * 1024;

/// The last bytes of a child's stderr.
#[derive(Debug)]
pub(crate) struct StderrTail {
    bytes: Vec<u8>,
    max_bytes: usize,
    elided: bool,
}

impl Default for StderrTail {
    fn default() -> Self {
        Self::with_max_bytes(MAX_STDERR_BYTES)
    }
}

impl StderrTail {
    pub(crate) fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
            elided: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= self.max_bytes {
            return;
        }
        let cut = ceil_utf8_boundary(&self.bytes, self.bytes.len() - self.max_bytes);
        self.bytes.drain(..cut);
        self.elided = true;
    }

    pub(crate) fn finish(self) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        let trimmed = text.trim();
        if self.elided {
            format!("{}{trimmed}", rho_sdk::ELLIPSIS)
        } else {
            trimmed.to_string()
        }
    }
}

/// First character start at or after `index`.
///
/// [`rho_sdk::ceil_char_boundary`] answers this for `&str`; the stderr tail is
/// cut while it is still raw bytes, before any decode, so the walk is over the
/// UTF-8 continuation-byte pattern instead.
fn ceil_utf8_boundary(bytes: &[u8], index: usize) -> usize {
    let mut index = index.min(bytes.len());
    while index < bytes.len() && bytes[index] & 0b1100_0000 == 0b1000_0000 {
        index += 1;
    }
    index
}

#[cfg(test)]
#[path = "stderr_tail_tests.rs"]
mod tests;
