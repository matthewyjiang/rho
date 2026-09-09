//! Official installer execution with process-tree supervision.
//! Windows upstream elevation can launch outside the supervised job.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{bail, Context};
use rho_sdk::CancellationToken;
use tokio::process::Command;

use crate::process_tree::{ProcessTree, SupervisedTree};

// Windows PowerShell uses .NET Framework, whose automatic redirects can downgrade
// HTTPS. Follow redirects explicitly and validate each URI before sending it.
const WINDOWS_INSTALL: &str = r#"
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Add-Type -AssemblyName System.Net.Http
$handler = New-Object Net.Http.HttpClientHandler
$handler.AllowAutoRedirect = $false
$client = New-Object Net.Http.HttpClient($handler)
$uri = [uri]'https://cua.ai/driver/install.ps1'
$visited = New-Object 'System.Collections.Generic.HashSet[string]'
try {
    while ($true) {
        if ($uri.Scheme -ne 'https') { throw 'Installer redirect must use HTTPS' }
        if (-not $visited.Add($uri.AbsoluteUri)) { throw 'Installer redirect cycle' }
        $response = $client.GetAsync($uri).GetAwaiter().GetResult()
        try {
            if ([int]$response.StatusCode -in 301,302,303,307,308) {
                if ($null -eq $response.Headers.Location) { throw 'Installer redirect has no location' }
                $uri = New-Object System.Uri($uri, $response.Headers.Location)
                continue
            }
            $null = $response.EnsureSuccessStatusCode()
            $script = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
            break
        } finally { $response.Dispose() }
    }
} finally { $client.Dispose() }
& ([scriptblock]::Create($script)) -NoPathUpdate -NoAutoStart
# Native stderr must not become a terminating PowerShell error. A missing
# executable must fail too, without inheriting the installer's last exit code.
$ErrorActionPreference = 'Continue'
$LASTEXITCODE = 1
& (Join-Path $env:CUA_DRIVER_RS_INSTALL_DIR 'cua-driver.exe') telemetry disable
exit $LASTEXITCODE
"#;

pub(super) fn command(home: &Path) -> anyhow::Result<Command> {
    let mut command = if cfg!(any(target_os = "linux", target_os = "macos")) {
        let mut command = Command::new("/bin/bash");
        // Fetch completely before executing: a failed/partial download never runs.
        command.args(["--noprofile", "--norc", "-c", "set -euo pipefail; script=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL https://cua.ai/driver/install.sh); /bin/bash --noprofile --norc -c \"$script\" cua-driver-install --no-modify-path --bin-dir \"$HOME/.local/bin\"; \"$HOME/.local/bin/cua-driver\" telemetry disable"]);
        command
    } else if cfg!(windows) {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_INSTALL]);
        command
    } else {
        bail!("automatic Cua Driver installation supports macOS, Linux and Windows only");
    };
    // Do not pass model credentials, shell startup hooks, or installer location
    // overrides into downloaded code. Proxy/certificate settings remain explicit
    // inputs. HOME is also the working directory, never the repo.
    command
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .current_dir(home);
    for key in [
        "PATH",
        "SYSTEMROOT",
        "PROCESSOR_ARCHITECTURE",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if cfg!(windows) {
        // The Windows installer owns a directory junction, not a shared bin dir.
        command.env(
            "CUA_DRIVER_RS_INSTALL_DIR",
            home.join(".cua-driver").join("bin"),
        );
    }
    Ok(command)
}

pub(super) fn with_log(mut command: Command) -> anyhow::Result<(Command, PathBuf)> {
    let (file, path) = tempfile::Builder::new()
        .prefix("rho-cua-install-")
        .suffix(".log")
        .tempfile()?
        .keep()?;
    command.stdout(file.try_clone()?).stderr(file);
    Ok((command, path))
}

pub(super) async fn run(
    mut command: Command,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    if cancellation.is_cancelled() {
        bail!("Cua Driver installation cancelled");
    }
    // Apply last so even caller opt-ins cannot enable installer hooks or the
    // post-install telemetry command. Both stages share supervision and the log.
    command
        .envs(super::super::policy::telemetry_environment())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    SupervisedTree::prepare(&mut command);
    let mut child = command
        .spawn()
        .context("could not start Cua Driver installer")?;
    let mut tree = match SupervisedTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            // In particular, reap a Windows child that could not join its job
            // while still suspended, rather than leaving cleanup to the runtime.
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(error).context("could not supervise Cua Driver installer");
        }
    };
    let status = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            tree.kill();
            let _ = child.wait().await;
            bail!("Cua Driver installation cancelled");
        }
        status = child.wait() => status?,
    };
    if !status.success() {
        bail!("Cua Driver installer exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "installer_tests.rs"]
mod tests;
