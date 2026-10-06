//! Cursor ACP fencing through a per-run, Rho-managed config directory.
//!
//! Spike recordings (2026-10-05) show that `--allowed-tools` and `--mode`
//! are ignored under ACP. The user's `approvalMode` controls whether Cursor
//! asks permission at all, so we copy their config and force `allowlist`.
//! Read/write/shell denies fence whole categories; grep/glob/ls cannot be
//! fenced. `WebFetch(*)` denies are ignored, so fetch is fenced by permission
//! answers instead. MCP permission classification is best effort, unverified.
//! Chats for these runs live in the Rho-managed directory, not the user's.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::{agent::CursorTool, permission::PermissionMode};

/// Config-only tripwire: local cli-config.json files measured 2,423 and 2,767
/// bytes on 2026-10-05. One MiB leaves over 370x headroom; allocation remains
/// proportional to the actual read. Exceeding it is an error, never truncation.
const CURSOR_CONFIG_READ_BUDGET_BYTES: u64 = 1024 * 1024;

/// Inject environment-derived paths; resolution itself never accesses env.
#[derive(Clone, Debug)]
pub(crate) struct CursorConfigPaths {
    pub(crate) cursor_config_dir: Option<PathBuf>,
    pub(crate) xdg_config_home: Option<PathBuf>,
    pub(crate) home: PathBuf,
}

impl CursorConfigPaths {
    /// The only environment-reading seam; the caller supplies resolved home.
    pub(crate) fn from_env(home: PathBuf) -> Self {
        Self {
            cursor_config_dir: std::env::var_os("CURSOR_CONFIG_DIR").map(PathBuf::from),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            home,
        }
    }

    pub(crate) fn user_dir(&self) -> PathBuf {
        let nonblank = |path: &&PathBuf| !path.as_os_str().to_string_lossy().trim().is_empty();
        if let Some(dir) = self.cursor_config_dir.as_ref().filter(nonblank) {
            dir.clone()
        } else if let Some(dir) = self.xdg_config_home.as_ref().filter(nonblank) {
            dir.join("cursor")
        } else {
            self.home.join(".cursor")
        }
    }
}

/// Cursor's actual enforcement units, not Rho's capability classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CursorCategory {
    Read,
    Search,
    Write,
    Shell,
    Fetch,
    Mcp,
    Inert,
}

impl CursorCategory {
    fn for_tool(tool: CursorTool) -> Self {
        match tool {
            CursorTool::Read => Self::Read,
            CursorTool::Grep
            | CursorTool::Glob
            | CursorTool::Ls
            | CursorTool::SemSearch
            | CursorTool::ReadLints => Self::Search,
            CursorTool::Edit | CursorTool::Delete | CursorTool::ApplyAgentDiff => Self::Write,
            CursorTool::Shell | CursorTool::WriteShellStdin => Self::Shell,
            CursorTool::WebFetch | CursorTool::Fetch | CursorTool::WebSearch => Self::Fetch,
            CursorTool::Mcp | CursorTool::ListMcpResources | CursorTool::ReadMcpResource => {
                Self::Mcp
            }
            CursorTool::UpdateTodos | CursorTool::ReadTodos | CursorTool::CreatePlan => Self::Inert,
        }
    }

    fn limitation(self) -> &'static str {
        match self {
            Self::Search => "search is unfenceable",
            Self::Inert => "session-artifact tools are not fenced",
            Self::Read => "coarsened to Read",
            Self::Write => "coarsened to Write",
            Self::Shell => "coarsened to Shell",
            Self::Fetch => "coarsened to Fetch permission requests",
            Self::Mcp => "coarsened to best-effort Mcp permission requests (unverified)",
        }
    }
}

/// A single visible notice groups all tool exclusions Cursor cannot enforce.
/// Keeping the data structured lets tests verify policy without locking copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorFenceNotice {
    pub(crate) exclusions: Vec<(CursorCategory, Vec<CursorTool>)>,
}

impl CursorFenceNotice {
    pub(crate) fn render(&self) -> String {
        let groups = self
            .exclusions
            .iter()
            .map(|(category, tools)| {
                let tools = tools
                    .iter()
                    .map(|tool| tool.as_flag())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{tools} ({})", category.limitation())
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("cursor ACP tools: exclusions cannot be enforced individually: {groups}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CursorFence {
    pub(crate) allow: Vec<String>,
    pub(crate) deny: Vec<&'static str>,
    /// Every category outside this set is rejected at the permission seam.
    pub(crate) allowed_categories: BTreeSet<CursorCategory>,
    pub(crate) notices: Vec<CursorFenceNotice>,
}

/// Derive whole-category config denies and permission gates from declared tools.
/// Unsupported modes fail closed; binding refuses them before this seam.
pub(crate) fn fence(mode: PermissionMode, tools: &[CursorTool]) -> CursorFence {
    let present: BTreeSet<_> = tools
        .iter()
        .copied()
        .map(CursorCategory::for_tool)
        .collect();
    let allowed_categories = match mode {
        PermissionMode::Bypass => present.clone(),
        PermissionMode::Plan
        | PermissionMode::Auto
        | PermissionMode::AllowEdits
        | PermissionMode::Supervised => BTreeSet::new(),
    };
    let writes_blocked = !matches!(mode, PermissionMode::Bypass);
    let mut deny = Vec::new();
    if !present.contains(&CursorCategory::Read) {
        deny.push("Read(**)");
    }
    if writes_blocked || !present.contains(&CursorCategory::Write) {
        deny.push("Write(**)");
    }
    if writes_blocked || !present.contains(&CursorCategory::Shell) {
        deny.push("Shell(*)");
    }

    let mut exclusions = Vec::new();
    for category in [
        CursorCategory::Search,
        CursorCategory::Write,
        CursorCategory::Shell,
        CursorCategory::Fetch,
        CursorCategory::Mcp,
        CursorCategory::Inert,
    ] {
        if matches!(category, CursorCategory::Search | CursorCategory::Inert)
            || allowed_categories.contains(&category)
        {
            let excluded: Vec<_> = CursorTool::ALL
                .iter()
                .copied()
                .filter(|tool| CursorCategory::for_tool(*tool) == category && !tools.contains(tool))
                .collect();
            if !excluded.is_empty() {
                exclusions.push((category, excluded));
            }
        }
    }
    CursorFence {
        allow: Vec::new(),
        deny,
        allowed_categories,
        notices: if exclusions.is_empty() {
            Vec::new()
        } else {
            vec![CursorFenceNotice { exclusions }]
        },
    }
}

/// Replace only permission policy, preserving auth/model/privacy and unknown keys.
/// A present non-object config is an error rather than a credential-losing default.
pub(crate) fn derive_config(
    user_config: Option<Value>,
    fence: &CursorFence,
) -> anyhow::Result<Value> {
    let mut config = user_config.unwrap_or_else(|| json!({}));
    let Some(object) = config.as_object_mut() else {
        bail!("cursor cli-config.json must be a JSON object");
    };
    object.insert("approvalMode".into(), json!("allowlist"));
    // Cursor approves web search from this flag without ever sending
    // `session/request_permission` (2026.10.01 `processPrompt`), which would
    // bypass Rho's Plan rejection and the declared-tools gate.
    object.insert("autoAcceptWebSearch".into(), json!(false));
    object.insert(
        "permissions".into(),
        json!({"allow": fence.allow, "deny": fence.deny}),
    );
    Ok(config)
}

#[derive(Debug)]
pub(crate) struct ManagedCursorConfig {
    /// Set CURSOR_CONFIG_DIR to this directory for the child only.
    pub(crate) dir: PathBuf,
    pub(crate) notices: Vec<String>,
}

/// Read the user's config and create a private, fresh per-run copy. Never
/// overwrite an existing managed directory (including symlinks). The run dir
/// must already exist; run ownership and cleanup belong to the session driver.
pub(crate) fn write_run_config(
    user_dir: &Path,
    run_dir: &Path,
    fence: &CursorFence,
) -> anyhow::Result<ManagedCursorConfig> {
    let source = user_dir.join("cli-config.json");
    let user_config = match File::open(&source) {
        Ok(file) => {
            let asked = file
                .metadata()
                .with_context(|| format!("could not stat {}", source.display()))?
                .len();
            if asked > CURSOR_CONFIG_READ_BUDGET_BYTES {
                bail!("Cursor config read budget exceeded for {}: limit {CURSOR_CONFIG_READ_BUDGET_BYTES} bytes, asked {asked} bytes", source.display());
            }
            let mut bytes = Vec::new();
            file.take(CURSOR_CONFIG_READ_BUDGET_BYTES + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("could not read {}", source.display()))?;
            if bytes.len() as u64 > CURSOR_CONFIG_READ_BUDGET_BYTES {
                let asked = bytes.len();
                bail!("Cursor config read budget exceeded for {}: limit {CURSOR_CONFIG_READ_BUDGET_BYTES} bytes, asked at least {asked} bytes", source.display());
            }
            Some(
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid JSON in {}", source.display()))?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", source.display()))
        }
    };
    let derived = derive_config(user_config, fence)
        .with_context(|| format!("invalid Cursor config at {}", source.display()))?;
    let bytes = serde_json::to_vec_pretty(&derived)?;
    let dir = run_dir.join("cursor-config");
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir).with_context(|| {
        format!(
            "could not create private Cursor config directory {}",
            dir.display()
        )
    })?;
    let target = dir.join("cli-config.json");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&target).with_context(|| {
        format!(
            "could not create private Cursor config {}",
            target.display()
        )
    })?;
    file.write_all(&bytes)
        .with_context(|| format!("could not write Cursor config {}", target.display()))?;
    Ok(ManagedCursorConfig {
        dir,
        notices: fence
            .notices
            .iter()
            .map(CursorFenceNotice::render)
            .collect(),
    })
}

#[cfg(test)]
#[path = "acp_config_tests.rs"]
mod tests;
