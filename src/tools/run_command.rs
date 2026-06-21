use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

pub struct RunCommand;
#[derive(Deserialize)]
struct Args {
    command: String,
    timeout_seconds: Option<u64>,
}

#[async_trait::async_trait]
impl Tool for RunCommand {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_command".into(),
            description: "Runs a shell command in the configured cwd.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"]}),
        }
    }
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let fut = Command::new("sh")
            .arg("-c")
            .arg(&args.command)
            .current_dir(&ctx.cwd)
            .output();
        let output = if let Some(secs) = args.timeout_seconds {
            tokio::time::timeout(std::time::Duration::from_secs(secs), fut)
                .await
                .map_err(|_| ToolError::Message(format!("command timed out after {secs}s")))??
        } else {
            fut.await?
        };
        let mut content = format!(
            "exit code: {}\n\nstdout:\n{}\n\nstderr:\n{}",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
            String::from_utf8(output.stdout)?,
            String::from_utf8(output.stderr)?
        );
        content = truncate(content, ctx.max_output_bytes);
        Ok(ToolResult {
            id,
            ok: output.status.success(),
            content,
        })
    }
}
