# Anthropic

Anthropic is one provider with API-key and OAuth auth modes. OAuth signs in with a Claude account and bills **usage credits** (extra usage), not the included Claude plan allowance. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

This page is the Rho **provider** path. It is not the Claude Code subscription runtime. For `runtime: claude-cli` agents, install the [Claude Code binary](/installation#claude-code-binary-optional), use [`/login claude-code`](/authentication-and-models#claude-code-runtime-sign-in), and follow [when this is useful and how to use it](/subagents/claude-cli).

## At a glance

| Method | Provider | Auth | Environment override |
| --- | --- | --- | --- |
| API key | `anthropic` | `anthropic-api-key` | `ANTHROPIC_API_KEY` |
| OAuth | `anthropic` | `anthropic-oauth` | `ANTHROPIC_ACCESS_TOKEN` |

Both modes use `https://api.anthropic.com/v1` and can refresh the provider model list after authentication.

## Sign in

Run `/login`, select **Anthropic**, then choose **API Key**, **OAuth**, or **Claude Code (delegation only)**. `/login anthropic` opens the same method picker. You can also target a method directly:

```text
/login anthropic-api-key
/login anthropic-oauth
```

API-key login opens a masked key entry box. `/login anthropic-oauth` opens the browser, then asks you to paste the authorization code from the callback page (`code#state`). If the browser does not launch, press `ctrl+y` in the code box to copy the login URL to your clipboard and open it by hand.

OAuth is usage-credits billing. Rho warns whenever that auth mode is signed in and selected: the session header, statusline (`usage credits`), `/info`, login, and auth-mode switch.

Credentials are stored in the configured credential store, not in config or transcripts.

## Sign out

Delete the stored credential for one method at a time:

```text
/logout anthropic-api-key
/logout anthropic-oauth
```

If the corresponding environment override is still present, that method stays available.

## Environment overrides

```bash
ANTHROPIC_API_KEY=...
ANTHROPIC_ACCESS_TOKEN=...
```

`ANTHROPIC_API_KEY` selects API-key authentication. `ANTHROPIC_ACCESS_TOKEN` is the OAuth CI/development override. Environment variables override stored credentials for their respective methods. For normal interactive setup, prefer `/login`.

## Models

Anthropic can refresh its provider model list through **Refresh model lists** in `/config`. Switch to an Anthropic model with:

```text
/model anthropic/claude-sonnet-4-5
```

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider anthropic --auth anthropic-api-key --model claude-sonnet-4-5 run "hello"
rho --provider anthropic --auth anthropic-oauth --model claude-sonnet-4-5 run "hello"
```

Provide the matching environment override or log in once through the TUI so Rho can read the stored credential.

## Notes

- OAuth spends usage credits (extra usage) at API rates. It does not draw from a Claude Pro/Max included allowance. Manage credits at [claude.ai/settings/usage](https://claude.ai/settings/usage).
- Claude Code subscription use stays on `/login claude-code` and `runtime: claude-cli`. Do not treat Anthropic OAuth as a subscription workaround.
