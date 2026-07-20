use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Renders paths consistently in user-facing text and structured output.
pub(crate) fn display(path: &Path) -> String {
    let rendered = path.to_string_lossy();
    #[cfg(windows)]
    {
        rendered.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        rendered.into_owned()
    }
}

/// Returns the user's home directory using platform-appropriate environment variables.
pub(crate) fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }

    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(profile));
        }

        if let (Some(drive), Some(path)) = (
            std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty()),
            std::env::var_os("HOMEPATH").filter(|value| !value.is_empty()),
        ) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Some(home);
        }
    }

    let _ = None::<OsString>;
    None
}
