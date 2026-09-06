//! Shell resolution and syntax shared by execution and path completion.

use std::path::Path;

pub(super) fn resolve_shell(configured: &str) -> String {
    if configured.trim().is_empty() {
        crate::config::default_inline_shell()
    } else {
        configured.to_string()
    }
}

#[derive(Clone, Copy)]
pub(super) enum ShellFamily {
    PosixSh,
    PosixLogin,
    PosixWrappedLogin,
    Fish,
    PowerShell,
    Cmd,
    Other,
}

impl ShellFamily {
    pub(super) fn for_executable(shell: &str) -> Self {
        let name = Path::new(shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(shell)
            .to_ascii_lowercase();
        match name.strip_suffix(".exe").unwrap_or(&name) {
            "powershell" | "pwsh" => Self::PowerShell,
            "cmd" => Self::Cmd,
            "sh" => Self::PosixSh,
            "bash" | "zsh" => Self::PosixWrappedLogin,
            "fish" => Self::Fish,
            "nu" | "nushell" | "csh" | "tcsh" | "xonsh" => Self::Other,
            // Preserve the runner's POSIX login-shell fallback for custom
            // shell executables, including ash, mksh, and busybox sh links.
            _ => Self::PosixLogin,
        }
    }

    pub(super) fn supports_path_completion(self) -> bool {
        match self {
            Self::PosixSh | Self::PosixLogin | Self::PosixWrappedLogin => true,
            Self::Fish | Self::PowerShell | Self::Cmd | Self::Other => false,
        }
    }
}

/// Bash and zsh preserve the parent PATH through the POSIX login wrapper.
/// Fish and other shells keep their unwrapped invocation.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ShellArgv {
    pub(super) args: Vec<String>,
    pub(super) carries_parent_path: bool,
}

impl ShellArgv {
    pub(super) fn for_shell(shell: &str, command: &str) -> Self {
        let plain = |args: &[&str]| Self {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            carries_parent_path: false,
        };
        match ShellFamily::for_executable(shell) {
            ShellFamily::PowerShell => plain(&["-NoLogo", "-NoProfile", "-Command", command]),
            ShellFamily::Cmd => plain(&["/C", command]),
            ShellFamily::PosixSh => plain(&["-c", command]),
            ShellFamily::PosixWrappedLogin => Self {
                args: vec!["-lc".into(), rho_tools::login_shell_script(command)],
                carries_parent_path: true,
            },
            ShellFamily::PosixLogin | ShellFamily::Fish | ShellFamily::Other => {
                plain(&["-lc", command])
            }
        }
    }
}
