use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

// Exact audited observation/input/navigation names from Cua Driver 0.23.2.
// Unknown future tools are denied. No setup, browser endpoint preparation,
// installation, recording, configuration, or session lifecycle RPCs.
pub(super) const ALLOWED_TOOLS: &[&str] = &[
    "bring_to_front",
    "browser_click",
    "browser_dialog",
    "browser_navigate",
    "browser_pointer",
    "browser_type",
    "click",
    "clipboard_read",
    "clipboard_write",
    "double_click",
    "drag",
    "get_accessibility_tree",
    "get_browser_state",
    "get_cursor_position",
    "get_desktop_state",
    "get_screen_size",
    "get_window_state",
    "hotkey",
    "invoke_menu",
    "list_apps",
    "list_windows",
    "press_key",
    "right_click",
    "scroll",
    "set_value",
    "set_window_frame",
    "type_text",
    "verify_state",
    "zoom",
];

pub(super) fn detect_driver(path: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    driver_candidates(path, home)
        .into_iter()
        .find(|candidate| executable(candidate))
        .and_then(|candidate| candidate.canonicalize().ok())
}

/// Only explicitly absolute installation locations may receive desktop authority.
pub(super) fn driver_candidates(path: Option<OsString>, home: Option<OsString>) -> Vec<PathBuf> {
    let executable_name = if cfg!(windows) {
        "cua-driver.exe"
    } else {
        "cua-driver"
    };
    let mut candidates: Vec<PathBuf> = path
        .as_deref()
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        // Never promote a repository executable through empty/relative PATH
        // entries into the host's desktop-authorized driver.
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(executable_name))
        .collect();
    if let Some(home) = home.map(PathBuf::from).filter(|home| home.is_absolute()) {
        candidates.push(home.join(".local/bin").join(executable_name));
    }
    candidates
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Desktop transport needs these ambient OS handles, but never provider secrets.
pub(super) fn desktop_environment() -> std::collections::BTreeMap<String, String> {
    [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "XAUTHORITY",
        "DBUS_SESSION_BUS_ADDRESS",
    ]
    .into_iter()
    .filter(|name| std::env::var_os(name).is_some())
    .map(|name| (name.to_owned(), name.to_owned()))
    .collect()
}
