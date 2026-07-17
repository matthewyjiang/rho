# GitHub Copilot

GitHub Copilot uses GitHub device-code auth. Rho talks to GitHub Copilot endpoints, not GitHub Models endpoints. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `github-copilot` |
| Auth | `github-copilot` |
| Environment override | `GITHUB_COPILOT_TOKEN` |
| API base | Dynamic, returned by the Copilot token exchange |
| Model list | Refreshable after authentication |

## Sign in

```text
/login github-copilot
```

`/login github-copilot` starts GitHub device code login. Credentials are stored in the native OS credential store, not in config or transcripts. Rho can exchange stored credentials for short-lived Copilot API tokens and refresh them once after an unauthorized response.

## Sign out

```text
/logout github-copilot
```

`/logout github-copilot` deletes stored GitHub Copilot tokens. If an environment override is still present, the provider stays available.

## Environment override

```bash
GITHUB_COPILOT_TOKEN=...
```

`GITHUB_COPILOT_TOKEN` is treated as a GitHub Copilot API bearer token. It is not refreshed or stored by Rho, and works as a CI/development override without storing credentials. For normal interactive setup, prefer `/login`.

## Models

GitHub Copilot exposes `github-copilot/<model>` names from the refreshed Copilot API model list. Fetch the list with `/refresh-model-list github-copilot` when credentials are available, then switch with:

```text
/model github-copilot/gpt-4.1
```

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider github-copilot --auth github-copilot --model gpt-4.1 run "hello"
```

## Automation

For non-interactive [`rho run`](/automation-cli) automation, first run `/login github-copilot` in the TUI or provide `GITHUB_COPILOT_TOKEN` as a bearer-token override, then select models as `github-copilot/<model>`.
