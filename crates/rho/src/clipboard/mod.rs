//! Shared clipboard facade for text write, image read, session policy, and doctor probes.

mod image;
mod process;
mod session;
mod write;

pub use image::{image_from_paste_text, read_clipboard_image};
pub use session::SessionKind;
pub use write::{CopyOutcome, SystemClipboard};

/// Doctor-facing snapshot of clipboard write and image-paste support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardDoctorReport {
    pub session_label: &'static str,
    pub text_write_status: &'static str,
    pub text_write_healthy: bool,
    pub text_write_detail: String,
    pub image_helpers: Vec<&'static str>,
}

impl ClipboardDoctorReport {
    pub fn image_status(&self) -> &'static str {
        if self.image_helpers.is_empty() {
            "not found"
        } else {
            "available"
        }
    }

    pub fn image_healthy(&self) -> bool {
        !self.image_helpers.is_empty()
    }

    pub fn image_detail(&self) -> String {
        if self.image_helpers.is_empty() {
            match self.session_label {
                "remote" => {
                    "Remote session detected. Image paste needs a local host clipboard helper and is unavailable over SSH/Mosh.".into()
                }
                _ => "Install a supported platform clipboard helper to paste images.".into(),
            }
        } else {
            format!("Detected: {}", self.image_helpers.join(", "))
        }
    }
}

pub fn doctor_report() -> ClipboardDoctorReport {
    let session = SessionKind::detect();
    let text_write = write::probe_text_write(session);
    ClipboardDoctorReport {
        session_label: session.label(),
        text_write_status: text_write.status,
        text_write_healthy: text_write.healthy,
        text_write_detail: text_write.detail,
        image_helpers: image::available_image_helpers(session),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn doctor_report_includes_session_and_text_write_fields() {
        let report = doctor_report();
        assert!(matches!(report.session_label, "local" | "remote" | "wsl"));
        assert!(!report.text_write_status.is_empty());
        assert!(!report.text_write_detail.is_empty());
    }

    #[test]
    fn empty_image_helpers_are_reported_as_missing() {
        let report = ClipboardDoctorReport {
            session_label: "local",
            text_write_status: "native",
            text_write_healthy: true,
            text_write_detail: "ok".into(),
            image_helpers: Vec::new(),
        };
        assert_eq!(report.image_status(), "not found");
        assert!(!report.image_healthy());
        assert!(report.image_detail().contains("Install"));
    }

    #[test]
    fn remote_image_detail_explains_session_limit() {
        let report = ClipboardDoctorReport {
            session_label: "remote",
            text_write_status: "osc 52",
            text_write_healthy: true,
            text_write_detail: "ok".into(),
            image_helpers: Vec::new(),
        };
        assert!(report.image_detail().contains("Remote session"));
    }
}
