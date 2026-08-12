# Cursor

Cursor uses Cursor's PKCE login and talks to Cursor's AgentService over Connect/protobuf. Rho owns tools; Cursor native tools are rejected so the model uses Rho MCP tools. MCP decode remaps Cursor-trained `block_until_ms` onto `timeout_seconds` and turns whole protobuf numbers into integers so bash parses. For shared concepts such as credential storage and model selection, see [authentication and models](/authentication-and-models).

## At a glance

| Setting | Value |
| --- | --- |
| Provider | `cursor` |
| Auth | `cursor-oauth` |
| Environment override | `CURSOR_ACCESS_TOKEN` |
| Login / token | `https://api2.cursor.sh` |
| AgentService | `https://agentn.global.api5.cursor.sh` (HTTP/2) |
| Model list | Refreshable after authentication |
| Default model | `auto` |

## Conversation

Each Rho model request is a new AgentService conversation. Rho rebuilds the transcript — including tool names, arguments, and results — and always sends a `UserMessageAction`. It does not reuse Cursor conversation ids or `ResumeAction`. Resuming a stream that ended on an MCP tool replayed the same read or edit.

## Sign in

```text
/login cursor
```

`/login cursor` opens Cursor's login page and waits until you approve it. In SSH and headless environments, Rho prints the URL instead of opening a browser. You can request that path explicitly:

```text
rho login cursor --device-auth
```

Credentials are stored in the configured credential store, not in config or transcripts. Rho refreshes stored tokens before expiry and once after an unauthorized response.

## Sign out

```text
/logout cursor
```

`/logout cursor` deletes stored Cursor tokens. If `CURSOR_ACCESS_TOKEN` is still set, the provider stays available.

## Environment override

```bash
CURSOR_ACCESS_TOKEN=...
```

`CURSOR_ACCESS_TOKEN` is treated as a Cursor API bearer token. It is not refreshed or stored by Rho, and works as a CI/development override without storing credentials. For normal interactive setup, prefer `/login`.

## Models

Cursor exposes `cursor/<model>` names from GetUsableModels, including `auto` for server-side routing. Fetch the list through **Refresh model lists** in `/config` when credentials are available, then switch with:

```text
/model cursor/auto
```

Fast variants are not separate picker rows. Cursor encodes Fast as a trailing `-fast` model id. Use `/fast on` or `/fast off` (or `/fast` to toggle). Rho saves the choice as `model.fast_mode`, shows `(fast)` after the active model name, and sends the `-fast` id on later turns. `auto` and product names such as `grok-code-fast-1` do not support the switch.

Reasoning works the same way. Cursor encodes effort as a trailing `-low`, `-medium`, `-high`, or `-xhigh` model id. Rho collapses those variants into one catalog row and puts only the levels it actually saw in `/config` → **Models & reasoning**. Grok 4.6 advertises `xhigh` when Cursor returns a `-xhigh` id. The selected level is written back onto the wire id, so Fast plus extra-high Grok is `grok-4.6-xhigh-fast`.

For a non-interactive run, pass the matching provider, auth mode, and model. These flags also update the persistent default:

```bash
rho --provider cursor --auth cursor-oauth --model auto run "hello"
```

If discovery fails with an empty model list, Rho falls back to a built-in Cursor model list and still exposes `auto`. Auth and transport errors from GetUsableModels are surfaced instead of looking like a healthy catalog.

## Automation

For non-interactive [`rho run`](/automation-cli) automation, first run `/login cursor` in the TUI or provide `CURSOR_ACCESS_TOKEN` as a bearer-token override, then select models as `cursor/<model>`.
