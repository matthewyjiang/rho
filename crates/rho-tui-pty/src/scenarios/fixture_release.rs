//! Explicit release signals for marker-gated provider fixtures.

/// Release the hanging compact fixture after the follow-up is queued.
///
/// Must match `RELEASE_MARKER` in `crates/rho-providers/src/providers/tui_fixture/compact.rs`.
pub(super) fn release_compact_fixture(
    harness: &mut crate::harness::PtyHarness,
) -> anyhow::Result<()> {
    release_fixture(harness, ".rho-fixture-release-compact")
}

pub(super) fn release_fixture(
    harness: &mut crate::harness::PtyHarness,
    marker: &str,
) -> anyhow::Result<()> {
    let cwd = harness
        .working_directory()
        .ok_or_else(|| anyhow::anyhow!("pty harness has no working directory"))?;
    std::fs::write(cwd.join(marker), b"")
        .map_err(|error| anyhow::anyhow!("write fixture release marker {marker}: {error}"))
}
