# TypeSafe

[TypeSafe](https://typesafe.ai/) hosts Jev, a decision model: it answers typed questions about a state with a probability for every option, rather than writing text. Rho uses TypeSafe only for decision models, such as the [permission classifier's screen](/configuration/permissions#screen-model). It is not a chat provider, so `/model` does not list it, and selecting it as the chat model is an error.

## Provider details

| Setting | Value |
| --- | --- |
| Provider | `typesafe` |
| Auth | `typesafe-api-key` |
| Environment override | `TYPESAFE_API_KEY` |
| API base | `https://api.typesafe.ai/v1` (System One API at `/systemone`) |
| Models | `jev-latest`, `jev-preview`, discovered from `/models` |

Create an API key in the TypeSafe dashboard, then store it with Rho. Do not put the key in `config.toml`.

## Interactive login

In the TUI, run:

```text
/login typesafe
```

Login discovers TypeSafe's models; **Refresh model lists** in `/config` updates them. Then pick Jev where a feature takes a decision model, for example **Permission screen model** in `/config`, or name it in config:

```toml
[internal_agents.permission-classifier-screen]
provider = "typesafe"
model = "jev-latest"
auth = "typesafe-api-key"
```

Remove the stored key with:

```text
/logout typesafe
```

## Environment and automation

For CI or headless runs, set `TYPESAFE_API_KEY`. A nonblank value takes precedence over the stored key.

## Limits

TypeSafe rejects a request past about 32,500 input tokens. Rho checks the state before sending, so a permission-screen transcript over about 20,500 estimated tokens is shortened, oldest tool calls first, or the screen is skipped and the review decides. Jev reports probabilities rounded to two decimals.
