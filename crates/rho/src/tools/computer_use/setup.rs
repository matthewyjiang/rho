//! Read-only onboarding: detection and next steps without starting the driver.

use super::{desktop_warning, detect_driver, ComputerUseSession};

impl ComputerUseSession {
    pub(crate) fn setup_guidance() -> String {
        let driver = detect_driver().map_or_else(
            || "Driver not detected. Install Cua Driver separately and make cua-driver available on PATH or at ~/.local/bin/cua-driver.".into(),
            |path| format!("Driver detected: {}", path.display()),
        );
        let permissions = if cfg!(target_os = "macos") {
            "Follow Cua's setup to grant Accessibility and Screen Recording permissions in macOS System Settings."
        } else {
            "Complete Cua's desktop permission setup for your platform. Launch Rho from the desktop session you intend to control."
        };
        let warning = desktop_warning()
            .map(|warning| format!("\n\nDisplay warning: {warning}"))
            .unwrap_or_default();
        format!(
            "Computer use setup\n\n1. Install and detect\n{driver}\nSetup guide: https://cua.ai/docs/how-to-guides/driver/connect-your-agent\n\n2. Check desktop permissions\n{permissions}\nDesktop capture and input permissions have not been checked by Rho.{warning}\n\n3. Allow access in Rho\nUse an image-capable model, then run /computer on and review the confirmation. Access includes signed-in apps, without per-action Rho approval even in supervised mode. Screenshots go to your model provider and session history.\n\n/computer opens the dashboard; /computer off revokes access. Rho does not install or update the driver, change permissions, or configure an MCP server for you."
        )
    }
}
