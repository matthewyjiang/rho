use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
};

use sha2::{Digest as _, Sha256};
use starlark::syntax::{AstModule, Dialect};

use super::{
    read_source_beneath, Budget, Digest, PlanningLimits, SourceFile, SourceManifest, WorkflowError,
    WorkflowResult,
};

#[derive(Clone, Debug)]
pub(crate) struct CollectedSources {
    pub(crate) entry_label: String,
    pub(crate) sources: BTreeMap<String, String>,
    pub(crate) manifest: SourceManifest,
}

impl CollectedSources {
    pub(crate) fn validate(&self, limits: &PlanningLimits) -> WorkflowResult<()> {
        limits.module_count.check(self.sources.len() as u64)?;
        if self.sources.keys().ne(self.manifest.modules.keys())
            || !self.sources.contains_key(&self.entry_label)
            || self.entry_label != self.manifest.entry_label
        {
            return Err(WorkflowError::Starlark(
                "collected source bytes do not match the source manifest".to_owned(),
            ));
        }
        let mut total = 0_u64;
        for (label, source) in &self.sources {
            total =
                total
                    .checked_add(source.len() as u64)
                    .ok_or(WorkflowError::BudgetExceeded {
                        budget: limits.total_source_bytes.name,
                        limit: limits.total_source_bytes.limit,
                        actual: u64::MAX,
                    })?;
            limits.total_source_bytes.check(total)?;
            let digest = Sha256::digest(source.as_bytes());
            let manifest = &self.manifest.modules[label];
            if manifest.bytes != source.len() as u64
                || manifest.digest.0 != format!("sha256:{digest:x}")
            {
                return Err(WorkflowError::Starlark(format!(
                    "collected source '{label}' does not match its authorized identity"
                )));
            }
        }
        Ok(())
    }
}

/// Opens root-relative workflow modules for collection.
///
/// Implementors enforce the trust boundary: the CLI uses a trusted local open,
/// while the tool path authorizes each path before opening a verified handle.
pub(crate) trait SourceBytes: Send {
    fn read_source<'a>(
        &'a mut self,
        root_relative: &'a Path,
        budget: &'a Budget,
        retained: u64,
    ) -> Pin<Box<dyn Future<Output = WorkflowResult<String>> + Send + 'a>>;
}

/// Trusted local reader that rejects symlinks and paths outside `root`.
pub(crate) struct LocalSourceBytes<'a> {
    root: &'a Path,
}

impl<'a> LocalSourceBytes<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self { root }
    }
}

impl SourceBytes for LocalSourceBytes<'_> {
    fn read_source<'a>(
        &'a mut self,
        root_relative: &'a Path,
        budget: &'a Budget,
        retained: u64,
    ) -> Pin<Box<dyn Future<Output = WorkflowResult<String>> + Send + 'a>> {
        Box::pin(async move {
            let path = checked_path(self.root, &self.root.join(root_relative))?;
            let opened_relative = path
                .strip_prefix(self.root)
                .map_err(|_| WorkflowError::SourceOutsideRoot { path: path.clone() })?;
            read_source_beneath(self.root, opened_relative, budget, retained)
        })
    }
}

pub(crate) struct SourceCollector<'a> {
    root: PathBuf,
    limits: &'a PlanningLimits,
}

impl<'a> SourceCollector<'a> {
    pub(crate) fn new(root: &Path, limits: &'a PlanningLimits) -> WorkflowResult<Self> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(WorkflowError::UntrustedDirectory(root));
        }
        Ok(Self { root, limits })
    }

    pub(crate) async fn collect(&self, entry: &Path) -> WorkflowResult<CollectedSources> {
        let mut reader = LocalSourceBytes::new(&self.root);
        self.collect_with(&mut reader, entry).await
    }

    /// Collect the entry module and every loaded dependency through `reader`.
    pub(crate) async fn collect_with<R: SourceBytes>(
        &self,
        reader: &mut R,
        entry: &Path,
    ) -> WorkflowResult<CollectedSources> {
        let relative = entry_relative(&self.root, entry)?;
        let entry_label = label_for(&relative)?;
        let mut sources = BTreeMap::new();
        let mut stack = Vec::new();
        let mut loaded = BTreeSet::new();
        let mut total_bytes = 0_u64;
        self.collect_label(
            reader,
            &entry_label,
            1,
            &mut stack,
            &mut loaded,
            &mut sources,
            &mut total_bytes,
        )
        .await?;
        let modules = sources
            .iter()
            .map(|(label, source)| {
                let digest = Sha256::digest(source.as_bytes());
                (
                    label.clone(),
                    SourceFile {
                        digest: Digest(format!("sha256:{digest:x}")),
                        bytes: source.len() as u64,
                    },
                )
            })
            .collect();
        Ok(CollectedSources {
            entry_label: entry_label.clone(),
            sources,
            manifest: SourceManifest {
                entry_label,
                modules,
            },
        })
    }

    fn collect_label<'b, R: SourceBytes>(
        &'b self,
        reader: &'b mut R,
        label: &'b str,
        depth: u64,
        stack: &'b mut Vec<String>,
        loaded: &'b mut BTreeSet<String>,
        sources: &'b mut BTreeMap<String, String>,
        total_bytes: &'b mut u64,
    ) -> Pin<Box<dyn Future<Output = WorkflowResult<()>> + Send + 'b>> {
        Box::pin(async move {
            self.limits.module_depth.check(depth)?;
            if loaded.contains(label) {
                return Ok(());
            }
            if let Some(index) = stack.iter().position(|item| item == label) {
                let mut cycle = stack[index..].to_vec();
                cycle.push(label.to_owned());
                return Err(WorkflowError::ImportCycle {
                    chain: cycle.join(" -> "),
                });
            }
            self.limits
                .module_count
                .check((loaded.len() + stack.len() + 1) as u64)?;
            let relative = validate_label(label)?;
            let source = reader
                .read_source(relative, &self.limits.total_source_bytes, *total_bytes)
                .await?;
            *total_bytes = total_bytes.checked_add(source.len() as u64).ok_or(
                WorkflowError::BudgetExceeded {
                    budget: self.limits.total_source_bytes.name,
                    limit: self.limits.total_source_bytes.limit,
                    actual: u64::MAX,
                },
            )?;
            self.limits.total_source_bytes.check(*total_bytes)?;
            let ast = AstModule::parse(label, source.clone(), &Dialect::Standard)
                .map_err(|error| WorkflowError::Starlark(error.to_string()))?;
            stack.push(label.to_owned());
            for load in ast.loads() {
                validate_label(load.module_id)?;
                self.collect_label(
                    reader,
                    load.module_id,
                    depth + 1,
                    stack,
                    loaded,
                    sources,
                    total_bytes,
                )
                .await?;
            }
            stack.pop();
            loaded.insert(label.to_owned());
            sources.insert(label.to_owned(), source);
            Ok(())
        })
    }
}

fn entry_relative(root: &Path, entry: &Path) -> WorkflowResult<PathBuf> {
    let entry = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        root.join(entry)
    };
    let relative = entry
        .strip_prefix(root)
        .map_err(|_| WorkflowError::SourceOutsideRoot {
            path: entry.clone(),
        })?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WorkflowError::SourceOutsideRoot { path: entry });
    }
    Ok(relative.to_path_buf())
}

fn checked_path(root: &Path, path: &Path) -> WorkflowResult<PathBuf> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| WorkflowError::SourceOutsideRoot {
            path: path.to_path_buf(),
        })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(WorkflowError::SourceOutsideRoot {
                path: path.to_path_buf(),
            });
        }
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(WorkflowError::SourceSymlink { path: current });
        }
    }
    let canonical = current.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(WorkflowError::SourceOutsideRoot { path: canonical });
    }
    if canonical.extension().and_then(|value| value.to_str()) != Some("star") {
        return Err(WorkflowError::InvalidModuleLabel {
            label: canonical.display().to_string(),
            reason: "module must use the .star extension".to_owned(),
        });
    }
    Ok(canonical)
}

pub(crate) fn validate_label(label: &str) -> WorkflowResult<&Path> {
    let invalid = || WorkflowError::InvalidModuleLabel {
        label: label.to_owned(),
        reason: "expected // followed by non-empty '/'-separated components and a .star suffix"
            .to_owned(),
    };
    if !label.starts_with("//") || label.contains('\\') || !label.ends_with(".star") {
        return Err(invalid());
    }
    let body = &label[2..];
    if body.is_empty()
        || body
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || body.contains(':')
    {
        return Err(invalid());
    }
    let path = Path::new(body);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid());
    }
    Ok(path)
}

fn label_for(path: &Path) -> WorkflowResult<String> {
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| WorkflowError::InvalidModuleLabel {
            label: path.display().to_string(),
            reason: "entry path is not portable UTF-8".to_owned(),
        })?;
    let label = format!("//{}", components.join("/"));
    validate_label(&label)?;
    Ok(label)
}

#[cfg(test)]
#[path = "starlark_loader_tests.rs"]
mod tests;
