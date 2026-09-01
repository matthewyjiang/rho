//! Doctor report model shared by the interactive overlay and `rho doctor`.
//!
//! Sections, check identities, and statuses are explicit enums so renderers
//! never infer meaning from strings. Probe rows start as `Checking`
//! placeholders and are replaced by identity once the probe finishes.

use serde::Serialize;

use super::plural_suffix;

/// Outcome of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DoctorStatus {
    /// Working as expected.
    Ok,
    /// Neutral state that needs no action, such as optional or not configured.
    Info,
    /// Degraded or missing, but Rho still runs.
    Warn,
    /// Broken; the related feature will not work.
    Fail,
    /// A probe is still running.
    Checking,
}

impl DoctorStatus {
    /// ASCII word for text output.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Checking => "checking",
        }
    }
}

/// Report sections in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DoctorSectionId {
    Authentication,
    Providers,
    Runtimes,
    Workspace,
    Extensions,
}

impl DoctorSectionId {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Authentication => "Authentication",
            Self::Providers => "Providers",
            Self::Runtimes => "Runtimes",
            Self::Workspace => "Workspace",
            Self::Extensions => "Extensions",
        }
    }
}

/// Stable identity of a check. Probe results replace rows by this id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DoctorCheckId {
    ProviderAuth { auth_mode: String },
    KeylessProvider { provider: String },
    ModelCache { provider: String },
    ProviderEndpoint { provider: String },
    SelectedModel,
    ClaudeAuth,
    ClaudeBinary,
    Rtk,
    Herdr,
    ConfigPath,
    SessionRoot,
    ClipboardText,
    ClipboardImage,
    Mcp,
    Plugins,
}

impl DoctorCheckId {
    pub(crate) const fn section(&self) -> DoctorSectionId {
        match self {
            Self::ProviderAuth { .. } | Self::KeylessProvider { .. } => {
                DoctorSectionId::Authentication
            }
            Self::ModelCache { .. } | Self::ProviderEndpoint { .. } | Self::SelectedModel => {
                DoctorSectionId::Providers
            }
            Self::ClaudeAuth | Self::ClaudeBinary | Self::Rtk | Self::Herdr => {
                DoctorSectionId::Runtimes
            }
            Self::ConfigPath | Self::SessionRoot | Self::ClipboardText | Self::ClipboardImage => {
                DoctorSectionId::Workspace
            }
            Self::Mcp | Self::Plugins => DoctorSectionId::Extensions,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DoctorCheck {
    pub(crate) id: DoctorCheckId,
    /// Row label, such as `OpenAI API key` or `Ollama connection`.
    pub(crate) label: String,
    pub(crate) status: DoctorStatus,
    /// Short lowercase state, such as `authenticated` or `unreachable`.
    pub(crate) summary: String,
    /// Detail or next step. The overlay shows it under issues only; text and
    /// JSON output always include it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hint: Option<String>,
}

impl DoctorCheck {
    pub(crate) fn new(
        id: DoctorCheckId,
        label: impl Into<String>,
        status: DoctorStatus,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            status,
            summary: summary.into(),
            hint: None,
        }
    }

    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DoctorSection {
    pub(crate) id: DoctorSectionId,
    pub(crate) checks: Vec<DoctorCheck>,
}

/// Sections in canonical order; empty sections are never present.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DoctorReport {
    pub(crate) sections: Vec<DoctorSection>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DoctorSummary {
    pub(crate) ok: usize,
    pub(crate) info: usize,
    pub(crate) warn: usize,
    pub(crate) fail: usize,
    pub(crate) checking: usize,
}

impl DoctorReport {
    pub(crate) fn from_checks(checks: Vec<DoctorCheck>) -> Self {
        let mut report = Self::default();
        for check in checks {
            report.push(check);
        }
        report
    }

    pub(crate) fn checks(&self) -> impl Iterator<Item = &DoctorCheck> {
        self.sections
            .iter()
            .flat_map(|section| section.checks.iter())
    }

    pub(crate) fn summary(&self) -> DoctorSummary {
        let mut summary = DoctorSummary::default();
        for check in self.checks() {
            match check.status {
                DoctorStatus::Ok => summary.ok += 1,
                DoctorStatus::Info => summary.info += 1,
                DoctorStatus::Warn => summary.warn += 1,
                DoctorStatus::Fail => summary.fail += 1,
                DoctorStatus::Checking => summary.checking += 1,
            }
        }
        summary
    }

    /// Replace every check with a matching id in place. Checks with a new id
    /// are appended to their section.
    pub(crate) fn replace_checks(&mut self, checks: Vec<DoctorCheck>) {
        for check in checks {
            let slot = self
                .sections
                .iter_mut()
                .flat_map(|section| section.checks.iter_mut())
                .find(|existing| existing.id == check.id);
            match slot {
                Some(slot) => *slot = check,
                None => self.push(check),
            }
        }
    }

    /// One-line summary: `all checks passed`, `2 failing · 1 warning`, with
    /// ` · checking 3` while probes are pending.
    pub(crate) fn headline(&self) -> String {
        let summary = self.summary();
        let mut parts = Vec::new();
        if summary.fail > 0 {
            parts.push(format!("{} failing", summary.fail));
        }
        if summary.warn > 0 {
            parts.push(format!(
                "{} warning{}",
                summary.warn,
                plural_suffix(summary.warn)
            ));
        }
        if summary.checking > 0 {
            parts.push(format!("checking {}", summary.checking));
        }
        if parts.is_empty() {
            return "all checks passed".into();
        }
        parts.join(" · ")
    }

    fn push(&mut self, check: DoctorCheck) {
        let section_id = check.id.section();
        if let Some(section) = self
            .sections
            .iter_mut()
            .find(|section| section.id == section_id)
        {
            section.checks.push(check);
            return;
        }
        let index = self
            .sections
            .iter()
            .position(|section| section.id > section_id)
            .unwrap_or(self.sections.len());
        self.sections.insert(
            index,
            DoctorSection {
                id: section_id,
                checks: vec![check],
            },
        );
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
