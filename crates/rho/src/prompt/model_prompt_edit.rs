//! Transactional filesystem work for model prompt editing. Editors run without a catalog lock.

use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tempfile::{NamedTempFile, TempDir};

use super::model_prompts;
use crate::model_identity::PromptModel;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EditOutcome {
    Saved(PathBuf),
    Unchanged,
}

pub(crate) struct ModelPromptEdit {
    home: PathBuf,
    provider: String,
    model: String,
    existing: Option<PathBuf>,
    original: Vec<u8>,
    draft: TempDir,
    draft_path: PathBuf,
}

impl ModelPromptEdit {
    pub(crate) fn prepare(home: &Path, provider: &str, model: &str) -> Result<Self> {
        let directory = home.join(".rho/model-prompts");
        fs::create_dir_all(&directory).with_context(|| {
            format!(
                "could not create model prompt directory {}",
                directory.display()
            )
        })?;
        let _lock = lock_catalog(&directory)?;
        let selected = model_prompts::load(Some(home), &identity(provider, model))?;
        let (existing, original) = if let Some(prompt) = selected {
            editable_metadata(&prompt.path)?;
            let bytes = fs::read(&prompt.path).with_context(|| {
                format!("could not read model prompt {}", prompt.path.display())
            })?;
            let source = std::str::from_utf8(&bytes).context("model prompt is not UTF-8")?;
            let (_, reread) = model_prompts::parse(&prompt.path, source)?;
            if reread.sha256 != prompt.sha256 {
                bail!(
                    "model prompt changed while preparing edit: {}",
                    prompt.path.display()
                );
            }
            (Some(prompt.path), bytes)
        } else {
            #[derive(Serialize)]
            struct Seed<'a> {
                provider: &'a str,
                model: &'a str,
                mode: &'static str,
            }
            let yaml = serde_yaml_ng::to_string(&Seed {
                provider,
                model,
                mode: "append",
            })?;
            let seed = format!("---\n{yaml}---\n\n");
            // Validate identifiers with the loader, without relaxing its nonempty-body rule.
            model_prompts::parse(Path::new("draft.md"), &format!("{seed}validation"))?;
            (None, seed.into_bytes())
        };
        let draft = tempfile::Builder::new()
            .prefix(".edit-")
            .tempdir_in(&directory)
            .with_context(|| {
                format!("could not create private draft in {}", directory.display())
            })?;
        let draft_path = draft.path().join("draft.md");
        fs::write(&draft_path, &original).context("could not write model prompt draft")?;
        Ok(Self {
            home: home.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            existing,
            original,
            draft,
            draft_path,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.draft_path
    }

    pub(crate) fn finish(self) -> Result<EditOutcome> {
        match self.commit() {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                let recovery = self.recover()?;
                Err(error).with_context(|| {
                    format!(
                        "model prompt was not saved; draft preserved at {}",
                        recovery.display()
                    )
                })
            }
        }
    }

    /// Keep the private directory on editor or validation failure, outside the live catalog.
    pub(crate) fn recover(self) -> Result<PathBuf> {
        Ok(self.draft.keep().join("draft.md"))
    }

    fn commit(&self) -> Result<EditOutcome> {
        let bytes = fs::read(self.path()).context("could not read edited model prompt draft")?;
        if bytes == self.original {
            return Ok(EditOutcome::Unchanged);
        }
        let source = std::str::from_utf8(&bytes).context("edited model prompt is not UTF-8")?;
        let (frontmatter, _) = model_prompts::parse(self.path(), source)?;
        if frontmatter.provider != self.provider || frontmatter.model != self.model {
            bail!(
                "edited model prompt must retain provider {} and model {}",
                self.provider,
                self.model
            );
        }
        let directory = self.home.join(".rho/model-prompts");
        let _lock = lock_catalog(&directory)?;
        let selected =
            model_prompts::load(Some(&self.home), &identity(&self.provider, &self.model))?;
        let (destination, permissions) = if let Some(path) = &self.existing {
            let metadata = editable_metadata(path)?;
            if selected.as_ref().map(|prompt| &prompt.path) != Some(path)
                || fs::read(path).with_context(|| format!("could not reread {}", path.display()))?
                    != self.original
            {
                bail!("model prompt changed during editing: {}", path.display());
            }
            (path.clone(), Some(metadata.permissions()))
        } else {
            if let Some(prompt) = selected {
                bail!(
                    "model prompt was created during editing: {}",
                    prompt.path.display()
                );
            }
            (
                available_path(&directory, &self.provider, &self.model)?,
                None,
            )
        };
        // A separate staging file keeps the user's draft intact if persistence fails.
        let mut staged = NamedTempFile::new_in(self.draft.path())
            .context("could not stage model prompt save")?;
        staged
            .write_all(&bytes)
            .context("could not write staged model prompt")?;
        if let Some(permissions) = permissions {
            staged
                .as_file()
                .set_permissions(permissions)
                .context("could not preserve model prompt permissions")?;
        }
        staged
            .as_file()
            .sync_all()
            .context("could not sync staged model prompt")?;
        if self.existing.is_some() {
            staged.persist(&destination)
        } else {
            staged.persist_noclobber(&destination)
        }
        .with_context(|| format!("could not save model prompt {}", destination.display()))?;
        Ok(EditOutcome::Saved(destination))
    }
}

fn identity(provider: &str, model: &str) -> PromptModel {
    PromptModel::Rho {
        provider: provider.to_owned(),
        model: model.to_owned(),
    }
}

fn editable_metadata(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect model prompt {}", path.display()))?;
    if metadata.is_symlink() {
        bail!(
            "refusing to replace symlink model prompt {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!("model prompt is not a regular file: {}", path.display());
    }
    if metadata.permissions().readonly() {
        bail!("model prompt is read-only: {}", path.display());
    }
    // A writable parent directory must not bypass the target's own write permissions or ACLs.
    OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("model prompt is not writable: {}", path.display()))?;
    Ok(metadata)
}

fn lock_catalog(directory: &Path) -> Result<File> {
    let path = directory.join(".edit.lock");
    if fs::symlink_metadata(&path)
        .is_ok_and(|metadata| metadata.is_symlink() || !metadata.is_file())
    {
        bail!(
            "model prompt lock is not a regular file: {}",
            path.display()
        );
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("could not open model prompt lock {}", path.display()))?;
    fs2::FileExt::try_lock_exclusive(&file).with_context(|| {
        format!(
            "could not acquire model prompt catalog lock {}; another edit may be saving",
            path.display()
        )
    })?;
    Ok(file)
}

fn safe_part(value: &str) -> String {
    let mut output = String::new();
    let mut replacing = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            output.push(ch);
            replacing = false;
        } else if !replacing {
            output.push('-');
            replacing = true;
        }
    }
    output
}

fn available_path(directory: &Path, provider: &str, model: &str) -> Result<PathBuf> {
    let mut stem = format!("{}_{}", safe_part(provider), safe_part(model));
    if stem.starts_with('.') {
        stem.insert(0, '_');
    }
    let mut suffix = None::<u64>;
    loop {
        let name = match suffix {
            None => format!("{stem}.md"),
            Some(number) => format!("{stem}-{number}.md"),
        };
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(path),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "could not inspect proposed model prompt path {}",
                        path.display()
                    )
                });
            }
            Ok(_) => {}
        }
        suffix = Some(match suffix {
            None => 2,
            Some(number) => number
                .checked_add(1)
                .context("model prompt filename suffix exhausted")?,
        });
    }
}

#[cfg(test)]
#[path = "model_prompt_edit_tests.rs"]
mod tests;
