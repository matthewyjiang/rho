# Agent Client Protocol

Parent: [Integrations](/integrations).

`rho acp` is a stdio JSON-RPC [Agent Client Protocol](https://agentclientprotocol.com/) agent. An editor host such as Zed starts the process and drives sessions over ACP.

```mermaid
flowchart TD
    host[Editor host] -->|stdio JSON-RPC| acp["rho acp"]
    acp --> store["~/.rho/sessions"]
    acp --> model[Process model and provider]
```

## Launch

```bash
rho acp
```

The process uses the model, provider, and other flags from the command line and config. There is no extra host plugin inside Rho.

stdout is protocol only. Logs go to stderr. Do not write other text to stdout, and do not point a logger at stdout while a host is connected.

## Sessions

ACP sessions persist under `~/.rho/sessions`. They appear in `rho sessions list` and you can resume them with `rho -R`. See [Sessions](/sessions).

## Permissions

New sessions advertise Rho permission modes: `bypass`, `auto`, `allow_edits`, `plan`, and `supervised`. `session/set_mode` is not supported yet.

When a tool needs approval, Rho asks the host. The host choices map to allow once, allow for this session, and reject.

`rho acp` can prompt the host for approvals. Headless `rho run` cannot.

## Model

`session/new` and `session/load` advertise a single `configOptions` select with id `model` and category `model`. Options are the same catalog list the TUI picker uses: favorites first, value ids `provider/model`.

Hosts switch with `session/set_config_option`. The value id is `provider/model`. A switch is rejected while a prompt is in flight. The new model applies to later prompts in that ACP session. It does not rewrite `config.toml`. `session/load` always resumes on the process default from config and flags, not on a model chosen earlier in that session.

## MCP

Rho still starts MCP servers from its own config. Host-supplied `mcpServers` are ignored. `rho acp` is not an MCP server. See [Model Context Protocol](/integrations/mcp).

## Automation

`rho acp` does not change `rho run --output jsonl`. Use [Automation and CLI](/automation-cli) for one-shot scripts.
