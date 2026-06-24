use std::{ffi::OsString, path::PathBuf};

/// Returns the user's home directory using platform-appropriate environment variables.
pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_from_env(|name| std::env::var_os(name))
}

fn home_dir_from_env(mut var: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(home) = var("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }

    #[cfg(windows)]
    {
        if let Some(profile) = var("USERPROFILE").filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(profile));
        }

        if let (Some(drive), Some(path)) = (
            var("HOMEDRIVE").filter(|value| !value.is_empty()),
            var("HOMEPATH").filter(|value| !value.is_empty()),
        ) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Some(home);
        }
    }

    None
}

pub(crate) fn rho_dir() -> anyhow::Result<PathBuf> {
    home_dir()
        .map(|home| home.join(".rho"))
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)], name: &str) -> Option<OsString> {
        vars.iter()
            .find_map(|(key, value)| (*key == name).then(|| OsString::from(value)))
    }

    #[test]
    fn uses_home_when_set() {
        assert_eq!(
            home_dir_from_env(|name| env(&[("HOME", "/home/rho")], name)),
            Some(PathBuf::from("/home/rho"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn falls_back_to_userprofile_on_windows() {
        assert_eq!(
            home_dir_from_env(|name| env(&[("USERPROFILE", r"C:\Users\rho")], name)),
            Some(PathBuf::from(r"C:\Users\rho"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn falls_back_to_homedrive_and_homepath_on_windows() {
        assert_eq!(
            home_dir_from_env(|name| {
                env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\rho")], name)
            }),
            Some(PathBuf::from(r"C:\Users\rho"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn does_not_use_windows_fallbacks_on_unix() {
        assert_eq!(
            home_dir_from_env(|name| env(&[("USERPROFILE", r"C:\Users\rho")], name)),
            None
        );
    }
}
