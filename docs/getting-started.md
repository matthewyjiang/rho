# Getting started

Rho is a fast Rust agent harness. Use it for an interactive coding session or a one-shot terminal prompt.

## Install and sign in

1. [Install Rho](/installation).
2. Set up [authentication and models](/authentication-and-models).
3. Run `rho`.

```text
rho
/login openai
/model openai/gpt-5.6-sol
```

For a one-shot prompt, pass the key in the environment and exit when the answer is done:

```bash
OPENAI_API_KEY=... rho run "summarize this repository"
```

`/model` and the matching CLI flags save the active provider and model. Other providers are on the [provider pages](/authentication-and-models#providers).

## Which surface

- [Interactive TUI](/interactive-tui) for an ongoing session. `rho --prompt "..."` opens that TUI with the first prompt already started.
- [Automation and CLI](/automation-cli) for a script, hook, or CI job.
- [Workflows](/workflows) for a fixed multi-step graph.

Config lives in `~/.rho/config.toml`. If setup fails, run `/doctor` or `rho doctor`.
