use std::{process::Stdio, time::Duration};

use tokio::{process::Command, time::timeout};

use crate::tool::ToolError;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) async fn run(command: &mut Command, description: &str) -> Result<(), ToolError> {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = command.spawn()?;
    let status = timeout(COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            ToolError::Message(format!(
                "{description} timed out after {}s",
                COMMAND_TIMEOUT.as_secs()
            ))
        })??
        .status;
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::Message(format!(
            "{description} failed with status {status}"
        )))
    }
}

pub(super) async fn output(
    command: &mut Command,
    description: &str,
) -> Result<std::process::Output, ToolError> {
    command.kill_on_drop(true);
    timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            ToolError::Message(format!(
                "{description} timed out after {}s",
                COMMAND_TIMEOUT.as_secs()
            ))
        })?
        .map_err(ToolError::Io)
}
