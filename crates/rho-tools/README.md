# rho-agent-tools

`rho-agent-tools` provides the workspace coding tools used by the Rho coding
agent and adapters for registering them with `rho-sdk`. The crate is imported as
`rho_agent_tools`.

The built-in tools cover `read_file`, `write`, one selectable edit surface,
`list_dir`, `grep`, and `glob`, with shared diff generation and output limiting.
`CodingToolOptions::edit_tool` selects `hashline` (`edit`), Codex-style
`apply_patch`, or `str_replace`; only that tool is registered.
`read_file` returns UTF-8 sources as hashline views. `grep` content mode also
mints `[path#TAG]` headers on matching files plus `N | text` match previews.
These snapshots let the default hashline `edit` chain tags and line numbers;
preview bodies are not exact source text. Successful mutations return bounded
post-edit snapshots, while full unified diffs stay in result metadata.
`coding_tools` constructs the SDK adapters, while `shell_tool` constructs the
platform shell adapter (`bash` on Linux and macOS, PowerShell on Windows).

The crate also exposes the application `Tool` contract and `RunCancellation` for
hosts that integrate with Rho's lower-level tool implementations. It is used by
the `rho` binary and can be used by embedders building their own agents on
`rho-sdk`.

The `document` module provides bounded extraction from paths or named byte
buffers. It supports UTF-8 text and source files directly,
text-layer PDFs as structured Markdown through `pdf-inspector`,
XLSX, XLS, and ODS through `calamine`, and a focused DOCX paragraph and table
walk. PDF stream expansion, including object and cross-reference streams, is
preflighted against a 64 MiB budget; chained or unbounded stream filters are
rejected. PDF, spreadsheet, and DOCX support are enabled by default and can be
controlled with the `document-pdf`, `document-spreadsheets`, and `document-docx`
features. The facade returns extracted text, MIME type, truncation state, and
warnings without exposing the format-specific parser APIs.

## Usage

Tools do not grant filesystem or process access when registered. Attach a
workspace and opt in to each required capability with a workspace policy.

```rust,no_run
use std::{error::Error, sync::Arc};

use rho_sdk::{provider::ModelProvider, Rho, ScopedWorkspacePolicy, Workspace};
use rho_agent_tools::{coding_tools, shell_tool, CodingToolOptions};

fn build_runtime(provider: Arc<dyn ModelProvider>) -> Result<Rho, Box<dyn Error>> {
    let workspace = Workspace::new(std::env::current_dir()?)?;
    let policy = ScopedWorkspacePolicy::new()
        .allow_read_paths()
        .allow_write_paths()
        .allow_processes();

    let mut builder = Rho::builder()
        .provider_shared(provider)
        .workspace(workspace)
        .workspace_policy(policy);

    for tool in coding_tools(CodingToolOptions::default()) {
        builder = builder.tool_shared(tool);
    }
    builder = builder.tool_shared(shell_tool(12_000));

    Ok(builder.build()?)
}
```

Hosts can add an approval handler and call `require_write_approval` or
`require_process_approval` on the policy when access should be confirmed at
runtime.

## License

Licensed under MIT AND Apache-2.0.
