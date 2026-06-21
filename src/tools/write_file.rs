use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

pub struct WriteFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".into(),
            description: "Writes a UTF-8 text file, creating or overwriting it.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        }
    }
    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, args.content)?;
        Ok(ToolResult {
            id,
            ok: true,
            content: format!("wrote {}", path.display()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_file_and_creates_parent_dirs() {
        let root = std::env::temp_dir().join(format!("rho-write-file-{}", uuid::Uuid::new_v4()));
        let ctx = ToolContext {
            cwd: root.clone(),
            max_output_bytes: 12000,
        };
        let result = WriteFile
            .call(
                json!({"path":"nested/hello.txt","content":"hello"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(root.join("nested/hello.txt")).unwrap(),
            "hello"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
