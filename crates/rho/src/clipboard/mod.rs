//! Shared clipboard facade for text write, image read, session policy, and doctor probes.

mod image;
mod path;
mod process;
mod session;
mod write;

pub use image::read_clipboard_image;
pub(crate) use image::{path_has_supported_image_magic, read_image_file};
pub(crate) use path::paste_text_as_file_path;
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
                "remote" => image::missing_image_helper_message(SessionKind::Remote),
                "wsl" => image::missing_image_helper_message(SessionKind::Wsl),
                _ => image::missing_image_helper_message(SessionKind::Local),
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

    // Covers: missing image helpers are unhealthy and explain the per-session
    // helper path rather than a generic clipboard error.
    // Owner: clipboard doctor report.
    #[test]
    fn empty_image_helpers_are_reported_as_missing() {
        for (session_label, session) in [
            ("local", SessionKind::Local),
            ("wsl", SessionKind::Wsl),
            ("remote", SessionKind::Remote),
        ] {
            let report = ClipboardDoctorReport {
                session_label,
                text_write_status: "native",
                text_write_healthy: true,
                text_write_detail: "ok".into(),
                image_helpers: Vec::new(),
            };
            assert!(!report.image_healthy(), "{session_label}");
            assert_eq!(
                report.image_detail(),
                image::missing_image_helper_message(session),
                "{session_label}"
            );
        }
    }
}
