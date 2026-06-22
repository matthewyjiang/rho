use crate::tool::*;
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

pub struct Bash;
#[derive(Deserialize)]
struct Args {
    command: String,
    timeout_seconds: Option<u64>,
}

#[async_trait::async_trait]
impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "Runs a bash command in the current working directory.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer"}},"required":["command"]}),
        }
    }

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_or_command()
    }

    fn display_command(&self, args: &serde_json::Value) -> Option<String> {
        args.get("command")
            .and_then(|command| command.as_str())
            .map(str::to_string)
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let start = std::time::Instant::now();
        let fut = Command::new("bash")
            .arg("-lc")
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
        let elapsed_secs = start.elapsed().as_secs_f64();
        let exit_code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        let mut content = format!(
            "stdout:\n{}\n\nstderr:\n{}\n\ntime: {:.1}s  exit code: {}",
            String::from_utf8(output.stdout)?,
            String::from_utf8(output.stderr)?,
            elapsed_secs,
            exit_code
        );
        content = truncate(content, ctx.max_output_bytes);
        Ok(ToolResult {
            id,
            ok: output.status.success(),
            content,
        })
    }
}
