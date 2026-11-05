//! Launch seams an external runtime session may replace.

use super::CliExecutable;

/// Seams a session may replace. Production leaves every field unset.
#[derive(Default)]
pub(crate) struct CliSessionOverrides {
    /// Binary to run instead of the resolved one.
    pub(crate) executable: Option<CliExecutable>,
    /// Frozen workflow identity arguments. Permission-sensitive flags are not
    /// reused as-is; launch regenerates those from the effective bound mode.
    pub(crate) frozen_argv: Option<Vec<String>>,
    /// Title generated for this run. The artifact sink reads it; the watch
    /// channel stays one-way.
    pub(crate) live_title: Option<crate::run_artifacts::LiveRunTitle>,
    /// A frozen caller can verify process facts and configure the child at the spawn boundary.
    pub(crate) before_spawn: Option<BeforeSpawn>,
}

pub(crate) type BeforeSpawn =
    Box<dyn Fn(&mut tokio::process::Command) -> std::io::Result<()> + Send + Sync>;
