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

/// Note appended to captured stderr when reading a child pipe fails, so a
/// truncated capture is never reported as the command's complete output.
pub(crate) fn capture_failure_notice(kind: StreamKind, error: &std::io::Error) -> String {
    format!("\n[rho: {} capture ended early: {error}]\n", kind.label())
}
