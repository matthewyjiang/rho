# Rho

Rho is a tiny YOLO coding agent harness for Rust.

## Usage

```bash
export OPENAI_API_KEY=...
cargo run --bin rho -- --model gpt-4.1-mini --cwd . --max-steps 8
```

Optional config:

```toml
model = "gpt-4.1-mini"
api_base = "https://api.openai.com/v1"
max_steps = 8
max_output_bytes = 12000
cwd = "."
```

Rho v0.1 intentionally has one OpenAI-compatible provider, prompt-level tool calls, five compiled-in tools, and no approvals, permissions, policies, allowlists, denylists, or sandboxing.
