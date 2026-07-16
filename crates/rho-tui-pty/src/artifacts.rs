//! Failure artifact capture for PTY scenarios.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::timing::TimingSummary;

#[derive(Clone, Debug, Serialize)]
pub struct ArtifactBundle {
    pub scenario: String,
    pub phase: String,
    pub message: String,
    pub rows: u16,
    pub cols: u16,
    pub exit_code: Option<u32>,
    pub action_log: Vec<String>,
    pub screen: String,
    pub timing: Option<TimingSummary>,
    pub env: Vec<(String, String)>,
}

/// Writes failure artifacts under a directory.
#[derive(Debug)]
pub struct ArtifactWriter {
    root: PathBuf,
}

impl ArtifactWriter {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write(&self, bundle: &ArtifactBundle, raw_output: &[u8]) -> Result<PathBuf> {
        fs::create_dir_all(&self.root).context("failed to create artifact root")?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let dir = self
            .root
            .join(format!("{}-{stamp}", sanitize(&bundle.scenario)));
        fs::create_dir_all(&dir)?;

        fs::write(dir.join("raw.pty"), raw_output)?;
        fs::write(dir.join("screen.txt"), &bundle.screen)?;
        fs::write(dir.join("actions.log"), bundle.action_log.join("\n") + "\n")?;
        let json = serde_json::to_vec_pretty(bundle)?;
        fs::write(dir.join("report.json"), json)?;
        Ok(dir)
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_bundle_files() {
        let temp = TempDir::new().unwrap();
        let writer = ArtifactWriter::new(temp.path());
        let bundle = ArtifactBundle {
            scenario: "startup stream".into(),
            phase: "wait_for_text".into(),
            message: "missing text".into(),
            rows: 24,
            cols: 80,
            exit_code: None,
            action_log: vec!["spawn".into(), "type".into()],
            screen: "rho".into(),
            timing: None,
            env: vec![("TERM".into(), "xterm-256color".into())],
        };
        let dir = writer.write(&bundle, b"raw-bytes").unwrap();
        assert!(dir.join("raw.pty").is_file());
        assert!(dir.join("screen.txt").is_file());
        assert!(dir.join("actions.log").is_file());
        assert!(dir.join("report.json").is_file());
    }
}
