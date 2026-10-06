//! Reservations for retained capture data and bounded journal serialization.

use super::*;

// Receipt: compact JSON map entries measured 158 bytes for an empty regular
// original and 175 for expected-after (u64::MAX size, Unix metadata). Round up
// to 256 to also reserve entry bookkeeping; charge path bytes separately.
const ENTRY_OVERHEAD_BYTES: u64 = 256;

pub(super) fn entry_bytes(path: &Path) -> u64 {
    (path.as_os_str().len() as u64).saturating_add(ENTRY_OVERHEAD_BYTES)
}

pub(super) fn encoded_content_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(3).saturating_mul(4)
}

impl OpenWorkspaceCheckpoint {
    /// Captures a caller-authorized path once. This method does no workspace authorization.
    pub(crate) fn capture_path(&mut self, path: &Path) -> CaptureDisposition {
        if self.quota_exceeded.is_some() {
            return CaptureDisposition::Paused;
        }
        if self.originals.contains_key(path) {
            return CaptureDisposition::AlreadyCaptured;
        }
        if !self.reserve_bytes(entry_bytes(path)) {
            return CaptureDisposition::Paused;
        }
        let remaining = self
            .max_session_bytes
            .saturating_sub(self.journal_bytes.saturating_add(self.captured_bytes));
        // Reserve base64's padded 4/3 expansion before reading the pre-image.
        let content_limit = (remaining / 4).saturating_mul(3);
        let mut state = capture_original(path, self.max_file_bytes.min(content_limit));
        let size = match &state {
            OriginalFileState::Regular(file) => file.bytes.len() as u64,
            OriginalFileState::Unsupported {
                reason: UnsupportedPath::TooLarge { size, .. },
            } if content_limit < self.max_file_bytes && *size <= self.max_file_bytes => {
                self.reserve_bytes(encoded_content_bytes(*size));
                return CaptureDisposition::Paused;
            }
            OriginalFileState::Absent | OriginalFileState::Unsupported { .. } => 0,
        };
        if let OriginalFileState::Unsupported {
            reason: UnsupportedPath::TooLarge { limit, .. },
        } = &mut state
        {
            *limit = self.max_file_bytes;
        }
        if !self.reserve_bytes(encoded_content_bytes(size)) {
            return CaptureDisposition::Paused;
        }
        self.originals.insert(path.to_path_buf(), state);
        CaptureDisposition::Captured
    }

    /// Reserve the second map entry before observing or retaining expected-after.
    pub(super) fn reserve_expected_after(&mut self, path: &Path) -> bool {
        if self.quota_exceeded.is_some() || !self.originals.contains_key(path) {
            return false;
        }
        self.expected_after.contains_key(path) || self.reserve_bytes(entry_bytes(path))
    }

    pub(crate) fn record_untracked_effect(&mut self, effect: UntrackedEffect) {
        if self.quota_exceeded.is_none()
            && !self.limitations.contains(&effect)
            && self.reserve_bytes((effect.source.len() as u64).saturating_add(ENTRY_OVERHEAD_BYTES))
        {
            self.limitations.push(effect);
        }
    }

    // Direct store users may not have an after-mutation observer. Reserve their
    // finalized states too, before constructing the file vector.
    pub(super) fn reserve_missing_expected_after(&mut self) {
        if self.quota_exceeded.is_some() {
            return;
        }
        let bytes = self
            .originals
            .keys()
            .filter(|path| !self.expected_after.contains_key(*path))
            .fold(0u64, |bytes, path| bytes.saturating_add(entry_bytes(path)));
        self.reserve_bytes(bytes);
    }

    fn reserve_bytes(&mut self, bytes: u64) -> bool {
        let turn_bytes = self.captured_bytes.saturating_add(bytes);
        let asked = self.journal_bytes.saturating_add(turn_bytes);
        if asked > self.max_session_bytes {
            self.quota_exceeded = Some(CheckpointAppendError::QuotaExceeded {
                asked,
                limit: self.max_session_bytes,
                turn_bytes,
            });
            self.originals.clear();
            self.expected_after.clear();
            self.limitations.clear();
            self.captured_bytes = 0;
            return false;
        }
        self.captured_bytes = turn_bytes;
        true
    }
}

struct BudgetWriter {
    bytes: Vec<u8>,
    remaining: u64,
    exceeded_at: Option<u64>,
}

impl Write for BudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let asked = (self.bytes.len() as u64).saturating_add(bytes.len() as u64);
        if asked > self.remaining {
            self.exceeded_at = Some(asked);
            return Err(std::io::Error::other(
                "checkpoint serialization budget exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Encode only up to the journal's remaining budget, without changing the journal.
pub(super) fn encode_record(
    checkpoint: &WorkspaceCheckpoint,
    journal_bytes: u64,
    limit: u64,
) -> Result<Vec<u8>, CheckpointAppendError> {
    let mut writer = BudgetWriter {
        bytes: Vec::new(),
        remaining: limit.saturating_sub(journal_bytes),
        exceeded_at: None,
    };
    let result = serde_json::to_writer(
        &mut writer,
        &StoredCheckpointRecord {
            version: CHECKPOINT_FORMAT_VERSION,
            checkpoint,
        },
    )
    .map_err(anyhow::Error::from)
    .and_then(|()| writer.write_all(b"\n").map_err(anyhow::Error::from));
    if let Some(turn_bytes) = writer.exceeded_at {
        return Err(CheckpointAppendError::QuotaExceeded {
            asked: journal_bytes.saturating_add(turn_bytes),
            limit,
            turn_bytes,
        });
    }
    result?;
    Ok(writer.bytes)
}
