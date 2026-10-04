//! Capture primitives shared by the shell tools that stream child output.

/// Identifies which child pipe a captured chunk came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamKind {
    Stdout,
    Stderr,
}

impl StreamKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// Last bytes of one child stream that fell past the retained output budget.
///
/// The model-facing capture keeps the start of the output. Readers of the
/// process result, such as lifecycle hooks, need its end, where errors usually
/// are, so this keeps at most `limit` of the dropped bytes. `lost` reports that
/// bytes between the retained start and this tail are gone.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StreamTail {
    bytes: Vec<u8>,
    limit: usize,
    lost: bool,
}

impl StreamTail {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            lost: false,
        }
    }

    pub(crate) fn push(&mut self, dropped: &[u8]) {
        self.bytes.extend_from_slice(dropped);
        let excess = self.bytes.len().saturating_sub(self.limit);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.lost = true;
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn lost(&self) -> bool {
        self.lost
    }
}

/// Note appended to captured stderr when reading a child pipe fails, so a
/// truncated capture is never reported as the command's complete output.
pub(crate) fn capture_failure_notice(kind: StreamKind, error: &std::io::Error) -> String {
    format!("\n[rho: {} capture ended early: {error}]\n", kind.label())
}
