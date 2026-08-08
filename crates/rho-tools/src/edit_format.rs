//! Canonical built-in file edit format registry.

use std::{fmt, str::FromStr, sync::Arc};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::sdk_adapter::build_edit_sdk_tool;

/// A built-in file edit surface exposed to models.
///
/// Hosts that import this type (or the `apply_patch` / `edit_file` modules) must
/// depend on a published `rho-agent-tools` version that exports them. Bump this
/// crate together with those consumers when the public edit surface changes so
/// crates.io package verification path-patches the unpublished cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditFormat {
    /// Snapshot-tagged, line-anchored `edit` tool.
    #[default]
    Hashline,
    /// Codex-compatible `apply_patch` tool.
    ApplyPatch,
    /// Exact string replacement through `edit_file`.
    EditFile,
}

impl EditFormat {
    /// Every supported edit format, in UI display order.
    pub const ALL: &'static [Self] = &[Self::Hashline, Self::ApplyPatch, Self::EditFile];

    /// The canonical model-facing tool name.
    pub const fn tool_name(self) -> &'static str {
        match self {
            Self::Hashline => "edit",
            Self::ApplyPatch => "apply_patch",
            Self::EditFile => "edit_file",
        }
    }

    /// The canonical configured value and model-facing tool name.
    pub const fn as_str(self) -> &'static str {
        self.tool_name()
    }
    /// The short human-readable format label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hashline => "Hash-line",
            Self::ApplyPatch => "Apply patch",
            Self::EditFile => "Replace string",
        }
    }

    /// The format detail shown when selecting an edit surface.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Hashline => {
                "Expose `edit` with snapshot tags and line-anchored PUT/CUT operations."
            }
            Self::ApplyPatch => {
                "Expose `apply_patch` with a Codex-style multi-file patch document."
            }
            Self::EditFile => "Expose `edit_file` with exact old_string/new_string replacement.",
        }
    }

    /// Resolves a canonical model-facing edit tool name.
    pub fn from_tool_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|format| format.tool_name() == name)
    }

    pub(crate) fn build_sdk_tool(
        self,
        max_output_bytes: usize,
        mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    ) -> Arc<dyn rho_sdk::tool::Tool> {
        build_edit_sdk_tool(self, max_output_bytes, mutation_observer)
    }
}

impl fmt::Display for EditFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.tool_name())
    }
}

impl FromStr for EditFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        Self::from_tool_name(&normalized).ok_or_else(|| {
            let expected = Self::ALL
                .iter()
                .copied()
                .map(Self::tool_name)
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown edit tool {normalized:?}; expected {expected}")
        })
    }
}

impl Serialize for EditFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.tool_name())
    }
}

impl<'de> Deserialize<'de> for EditFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
