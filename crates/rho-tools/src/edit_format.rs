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
///
/// Config identity, selector label, and model-facing tool name share one
/// vocabulary per format: `hashline` exposes `edit`, `apply_patch` exposes
/// `apply_patch`, and `str_replace` exposes `str_replace`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditFormat {
    /// Snapshot-tagged, line-anchored `edit` tool.
    #[default]
    Hashline,
    /// Codex-compatible `apply_patch` tool.
    ApplyPatch,
    /// Exact string replacement through the `str_replace` tool.
    StrReplace,
}

impl EditFormat {
    /// Every supported edit format, in UI display order.
    pub const ALL: &'static [Self] = &[Self::Hashline, Self::ApplyPatch, Self::StrReplace];

    /// The canonical model-facing tool name.
    ///
    /// Each format has a unique name so hosts classify by name alone.
    pub const fn tool_name(self) -> &'static str {
        match self {
            Self::Hashline => "edit",
            Self::ApplyPatch => "apply_patch",
            Self::StrReplace => "str_replace",
        }
    }

    /// The canonical configured value and selector label (`behavior.edit_tool`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hashline => "hashline",
            Self::ApplyPatch => "apply_patch",
            Self::StrReplace => "str_replace",
        }
    }

    /// The short human-readable format label shown in selectors.
    ///
    /// Same string as [`Self::as_str`].
    pub const fn label(self) -> &'static str {
        self.as_str()
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
            Self::StrReplace => {
                "Expose `str_replace` with exact old_string/new_string replacement."
            }
        }
    }

    /// Resolves a configured `behavior.edit_tool` value.
    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "hashline" => Some(Self::Hashline),
            "apply_patch" => Some(Self::ApplyPatch),
            "str_replace" => Some(Self::StrReplace),
            _ => None,
        }
    }

    /// Whether `name` is a model-facing built-in edit tool name.
    ///
    /// Includes the legacy `edit_file` name so older transcripts and agent
    /// frontmatter still classify as edit.
    pub fn is_edit_tool_name(name: &str) -> bool {
        matches!(name, "edit" | "apply_patch" | "str_replace" | "edit_file")
    }

    /// Resolves a model-facing edit tool name.
    ///
    /// Names are unique per format. The legacy model-facing name `edit_file`
    /// still maps to [`Self::StrReplace`].
    pub fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "edit" => Some(Self::Hashline),
            "apply_patch" => Some(Self::ApplyPatch),
            "str_replace" | "edit_file" => Some(Self::StrReplace),
            _ => None,
        }
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
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EditFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        Self::from_config_value(&normalized).ok_or_else(|| {
            let expected = Self::ALL
                .iter()
                .copied()
                .map(Self::as_str)
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
        serializer.serialize_str(self.as_str())
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
