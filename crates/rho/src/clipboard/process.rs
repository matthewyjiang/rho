use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

/// Returns true when the OS can resolve and spawn `command`.
///
/// Exit status is ignored: many helpers reject `--help` or return non-zero while
/// still being installed and usable.
pub(super) fn command_available(command: &str) -> bool {
    Command::new(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .arg("--help")
        .output()
        .is_ok()
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

    let write_result = (|| {
        let mut stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, format!("{program} stdin closed"))
        })?;
        stdin.write_all(bytes)?;
        stdin.flush()?;
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited with {status}")))
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
