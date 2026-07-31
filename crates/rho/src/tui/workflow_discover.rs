//! Discover workspace workflow entry files for the in-app hub.

use std::path::{Path, PathBuf};

/// A workflow source the hub can validate or plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiscoveredWorkflow {
    /// Workspace-relative path using `/` separators.
    pub(super) relative_path: String,
    /// Absolute path to the entry `.star` file.
    pub(super) absolute_path: PathBuf,
    /// Short label for picker rows.
    pub(super) label: String,
}

/// Finds workflow entry files under `.rho/workflows`.
///
/// Accepts:
/// - `.rho/workflows/<name>.star`
/// - `.rho/workflows/<name>/workflow.star`
///
/// Nested helpers such as `common.star` are skipped unless they are the only
/// entry shape for a folder named after the file.
pub(super) fn discover_workflow_sources(workspace: &Path) -> Vec<DiscoveredWorkflow> {
    let root = workspace.join(".rho").join("workflows");
    if !root.is_dir() {
        return Vec::new();
    }

    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            if is_star_file(&path) {
                push_discovered(workspace, path, &mut found);
            }
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        let entry_star = path.join("workflow.star");
        if entry_star.is_file() {
            push_discovered(workspace, entry_star, &mut found);
        }
    }

    found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    found
}

fn is_star_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("star")
}

fn push_discovered(workspace: &Path, absolute: PathBuf, out: &mut Vec<DiscoveredWorkflow>) {
    let Ok(relative) = absolute.strip_prefix(workspace) else {
        return;
    };
    let relative_path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if relative_path.is_empty() {
        return;
    }
    let label = workflow_label(&relative_path);
    out.push(DiscoveredWorkflow {
        relative_path,
        absolute_path: absolute,
        label,
    });
}

fn workflow_label(relative_path: &str) -> String {
    let path = Path::new(relative_path);
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "workflow.star")
    {
        if let Some(parent) = path.parent().and_then(|parent| parent.file_name()) {
            return parent.to_string_lossy().into_owned();
        }
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative_path.to_owned())
}

#[cfg(test)]
#[path = "workflow_discover_tests.rs"]
mod tests;
