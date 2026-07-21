use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

/// Returns true when `command` resolves to an executable on `PATH`.
///
/// The command is never spawned: some clipboard helpers copy their stdin to the
/// clipboard regardless of arguments (`clip.exe` ignores `--help` and copies
/// stdin; `xclip` reads stdin into the primary selection), so executing them to
/// probe availability would mutate the clipboard.
pub(super) fn command_available(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    command_available_in(command, std::env::split_paths(&path))
}

fn command_available_in(command: &str, dirs: impl Iterator<Item = PathBuf>) -> bool {
    dirs.flat_map(|dir| executable_candidates(&dir, command))
        .any(|candidate| is_executable_file(&candidate))
}

/// The paths to test for `command` in `dir`. On Windows a bare name resolves
/// through the common executable extensions, matching PATHEXT resolution.
fn executable_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    let mut candidates = vec![dir.join(command)];
    if cfg!(windows) && Path::new(command).extension().is_none() {
        for extension in ["exe", "cmd", "bat"] {
            candidates.push(dir.join(format!("{command}.{extension}")));
        }
    }
    candidates
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    // WSL presents Windows .exe helpers under /mnt/c with the DrvFs default 0777,
    // so the exec bit holds unless the mount clears it with a custom fmask.
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub(super) fn command_output(command: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

pub(super) fn write_command_stdin(program: &str, args: &[&str], bytes: &[u8]) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| io::Error::new(error.kind(), format!("spawn {program}: {error}")))?;

    let write_result: io::Result<()> = (|| {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, format!("{program} stdin closed"))
        })?;
        stdin.write_all(bytes)?;
        stdin.flush()?;
        Ok(())
    })();

    // A non-zero exit is authoritative and deterministic regardless of whether
    // the write raced the child closing stdin. On a clean exit the write result
    // decides success, so an incomplete write (broken pipe) is still an error
    // and the caller can fall back rather than assume the clipboard was set.
    let status = child.wait()?;
    resolve_command_write(program, status, write_result)
}

fn resolve_command_write(
    program: &str,
    status: ExitStatus,
    write_result: io::Result<()>,
) -> io::Result<()> {
    if !status.success() {
        return Err(io::Error::other(format!("{program} exited with {status}")));
    }
    write_result
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
