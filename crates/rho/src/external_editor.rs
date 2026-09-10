//! Editor selection and process setup shared by CLI and suspended TUI editing.

use anyhow::{anyhow, Context};
use std::{
    ffi::{OsStr, OsString},
    path::Path,
};
use tokio::process::Command;

/// Prefer VISUAL, then EDITOR. Do not invent a platform default editor.
pub(crate) fn resolve_editor(
    visual: Option<OsString>,
    editor: Option<OsString>,
) -> Option<OsString> {
    match visual {
        Some(value) if !value.is_empty() => Some(value),
        _ => editor.filter(|value| !value.is_empty()),
    }
}

/// Parse executable and arguments without evaluating shell commands.
pub(crate) fn editor_command(editor: &OsStr) -> anyhow::Result<Command> {
    let parts = editor_parts(editor)?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| anyhow!("editor command is empty"))?;
    if program.is_empty() {
        return Err(anyhow!("editor command is empty"));
    }
    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

fn editor_parts(editor: &OsStr) -> anyhow::Result<Vec<OsString>> {
    if editor.is_empty() {
        return Err(anyhow!("editor command is empty"));
    }
    if Path::new(editor).is_file() {
        return Ok(vec![editor.to_os_string()]);
    }
    split_editor_command(editor)
}

#[cfg(unix)]
fn split_editor_command(editor: &OsStr) -> anyhow::Result<Vec<OsString>> {
    let editor = editor
        .to_str()
        .context("editor command is not valid UTF-8 and is not an executable path")?;
    shell_words::split(editor)
        .context("editor command has invalid quoting")
        .map(|parts| parts.into_iter().map(OsString::from).collect())
}

#[cfg(windows)]
fn split_editor_command(editor: &OsStr) -> anyhow::Result<Vec<OsString>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};

    let mut command_line = editor.encode_wide().collect::<Vec<_>>();
    command_line.push(0);
    let mut count = 0;
    let arguments = unsafe { CommandLineToArgvW(command_line.as_ptr(), &mut count) };
    if arguments.is_null() {
        return Err(std::io::Error::last_os_error()).context("could not parse editor command");
    }
    let pointers = unsafe { std::slice::from_raw_parts(arguments, count as usize) };
    let parts = pointers
        .iter()
        .map(|argument| {
            let argument = *argument;
            let mut len = 0;
            while unsafe { *argument.add(len) } != 0 {
                len += 1;
            }
            let value = unsafe { std::slice::from_raw_parts(argument, len) };
            OsString::from_wide(value)
        })
        .collect();
    unsafe {
        LocalFree(arguments.cast());
    }
    Ok(parts)
}

/// Let the foreground child handle terminal interrupts without killing the host.
#[cfg(unix)]
pub(crate) mod unix_suspended_child_signals {
    use std::{io, mem::MaybeUninit, os::unix::process::CommandExt};
    use tokio::process::Command;

    const PARENT_IGNORED_SIGNALS: [libc::c_int; 2] = [libc::SIGINT, libc::SIGQUIT];
    const CHILD_DEFAULT_SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP];

    pub struct SuspendedChildSignalGuard {
        previous: Vec<(libc::c_int, libc::sigaction)>,
    }

    impl SuspendedChildSignalGuard {
        pub fn install(command: &mut Command) -> io::Result<Self> {
            let mut previous = Vec::with_capacity(PARENT_IGNORED_SIGNALS.len());
            for signal in PARENT_IGNORED_SIGNALS {
                match replace_handler(signal, libc::SIG_IGN) {
                    Ok(action) => previous.push((signal, action)),
                    Err(error) => {
                        restore_handlers(&previous);
                        return Err(error);
                    }
                }
            }
            unsafe {
                command.as_std_mut().pre_exec(|| {
                    for signal in CHILD_DEFAULT_SIGNALS {
                        replace_handler(signal, libc::SIG_DFL)?;
                    }
                    Ok(())
                });
            }
            Ok(Self { previous })
        }
    }
    impl Drop for SuspendedChildSignalGuard {
        fn drop(&mut self) {
            restore_handlers(&self.previous);
        }
    }
    fn replace_handler(
        signal: libc::c_int,
        handler: libc::sighandler_t,
    ) -> io::Result<libc::sigaction> {
        let mut replacement = unsafe { std::mem::zeroed::<libc::sigaction>() };
        replacement.sa_sigaction = handler;
        unsafe {
            libc::sigemptyset(&mut replacement.sa_mask);
        }
        let mut previous = MaybeUninit::<libc::sigaction>::uninit();
        if unsafe { libc::sigaction(signal, &replacement, previous.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { previous.assume_init() })
    }
    fn restore_handlers(previous: &[(libc::c_int, libc::sigaction)]) {
        for (signal, action) in previous.iter().rev() {
            unsafe {
                libc::sigaction(*signal, action, std::ptr::null_mut());
            }
        }
    }
}

#[cfg(test)]
#[path = "external_editor_tests.rs"]
mod tests;
