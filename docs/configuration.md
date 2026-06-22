# Configuration

Rho stores persistent config at `~/.rho/config.toml` by default.

```toml
provider = "openai"
model = "gpt-5.5"
max_output_bytes = 12000
auth = "api-key" # or "codex"
reasoning_effort = "medium" # set to "none" to omit
reasoning_summary = "auto"  # set to "none" to omit
```

## CLI overrides

Passing `--provider`, `--model`, or `--auth` updates the config file and makes the choice the future default.

```bash
rho --provider openai --auth api-key --model gpt-5.5
rho --provider openai-codex --auth codex --model gpt-5.5
```

These values select [authentication and models](/authentication-and-models).

You can load and save a specific config file with:

```bash
rho --config ~/.rho/config.toml
```

## TUI updates

In the [interactive TUI](/interactive-tui), [`/login`](/interactive-tui#commands) stores credentials in the OS credential store, not in this config file. [`/logout`](/interactive-tui#commands) deletes stored credentials. [`/model`](/interactive-tui#commands) saves the selected `provider` and `model`. The picker shows entries from Rho's static [model catalog](/authentication-and-models#providers-and-model-catalog) for providers with available auth, and `/model provider/model` can switch explicitly.

## Reasoning options

`reasoning_effort` and `reasoning_summary` are passed to supported providers. Set either value to `none` to omit that option from provider requests.

## Tool output limit

`max_output_bytes` controls how much output Rho keeps from [tool](/tools-workspace) calls such as command output and file reads.
