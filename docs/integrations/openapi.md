# OpenAPI

Rho can turn an [OpenAPI](https://www.openapis.org/) 3.0 or 3.1 spec into read-only REST tools. Each `GET` and `HEAD` operation becomes one tool. Write methods (`POST`, `PUT`, `PATCH`, `DELETE`) are never registered.

The tools stay out of model context until something loads them, the same as MCP tools. The model finds them with `tool_search`, or discovers and calls them inside `codemode`. The system prompt gets one line per source:

```text
- github: 650 read operations, 43 tags (repos, actions, orgs, issues, users, activity, …)
```

## Configuration

Add sources under `[openapi.<source>]` in `~/.rho/config.toml`:

```toml
[openapi.github]
spec = "https://raw.githubusercontent.com/github/rest-api-description/main/descriptions/api.github.com/api.github.com.json"
base_url = "https://api.github.com"
headers = { X-GitHub-Api-Version = "2022-11-28" }
auth = { type = "bearer", token_env = "GITHUB_TOKEN" }
deny = ["GET /orgs/*/audit-log"]
```

| Key | Meaning |
| --- | --- |
| `spec` | HTTP(S) URL or file path of a JSON or YAML spec. A relative path resolves against the config file's directory. |
| `base_url` | Replaces every server the spec declares. Without it, calls use the operation's or the spec's first usable server. Required when the source sends credentials or headers. |
| `headers` | Literal headers sent on every call to the source's origin. |
| `headers_from_env` | Header names mapped to environment variable names, for secrets. |
| `auth` | One credential, read from the environment. See [Authentication](#authentication). |
| `allow` | Globs on the operationId or `METHOD /path`. Empty allows every read operation. |
| `deny` | Globs on the operationId or `METHOD /path`, checked before `allow`. |
| `timeout_secs` | Budget for one call, redirects and body included, and for the spec download. Default 120, the same as an MCP tool call. |

The source name must contain only ASCII letters and digits, optionally joined by single underscores. Tool names are `openapi__<source>__<operationId>`.

In globs, `*` matches any run of characters, `/` included, and `?` matches one character. Matching is case-sensitive, and methods are uppercase: `GET /repos/*`.

Values are taken literally. Rho does not expand `${VAR}`; use `headers_from_env` and `auth` for anything secret.

### Authentication

```toml
auth = { type = "bearer", token_env = "GITHUB_TOKEN" }
auth = { type = "basic", username_env = "API_USER", password_env = "API_PASSWORD" }
auth = { type = "api_key", in = "header", name = "X-Api-Key", value_env = "API_KEY" }
auth = { type = "api_key", in = "query", name = "api_key", value_env = "API_KEY" }
```

Credentials and headers, literal ones included, go only to the source's origin: `base_url`, or the spec's first server when `base_url` is unset. A source that sends credentials or headers must set `base_url`, and it must use HTTPS unless its host is loopback. When a call is redirected to another origin, Rho follows the redirect without any of them. A missing environment variable fails that source at session start.

### Project sources

A repository can declare sources in `<project root>/.rho/openapi.toml`, using the same tables. The project root is the Git root, or the working directory outside Git.

```toml
[openapi.petstore]
spec = "specs/petstore.yaml"
base_url = "http://localhost:8080"
```

A project file cannot attach credentials. A project source that sets `auth` or `headers_from_env` fails with an error, and a user source with the same name replaces the project source entirely. A repository therefore can never send your tokens to a server it chose. Relative `spec` paths resolve against the project root.

Project sources load only in trusted workspaces. A project source makes Rho download its spec and lets the model call read operations against the declared server without approval, which may be a local or internal address. Until you opt in, Rho ignores the file and shows a warning:

```bash
RHO_TRUST_PROJECT_OPENAPI=1 rho
```

This is the same family as `RHO_TRUST_PROJECT_HOOKS`, `RHO_TRUST_PROJECT_AGENTS`, and `RHO_TRUST_PROJECT_PLUGINS`.

## Loading

Interactive sessions load sources in the background, together with MCP servers, so a large spec does not delay the first frame. A prompt sent before loading finishes waits for it. Headless runs, ACP sessions, and subagents load sources before they start.

Downloaded specs are cached in `~/.rho/cache/openapi/` and revalidated with their `ETag` every session. If a download fails and a cached copy exists, Rho uses the cached copy and shows a warning. A source that fails to load shows an error in the transcript; the others still load. Headless runs log these with `RHO_LOG=warn`.

Specs larger than 64 MiB are rejected. Operations Rho cannot call correctly, such as those with cookie parameters or non-JSON request bodies, are skipped.

## Calls

Path, query, and header parameters are top-level arguments. A name used in two locations gets the location as a prefix, as in `path_id` and `query_id`. A JSON request body goes under `body`.

Arguments only fill in values. The origin and path come from config and the spec, path values are percent-encoded, and a full URL cannot be passed. Arguments are checked before any credential is attached.

Calls use the `network` capability, which every [permission mode](/configuration/permissions) allows. The URL each call authorizes is the one built from its arguments.

A call returns this to scripts:

```json
{
  "status": 200,
  "body": [{ "number": 1 }],
  "next": { "owner": "rust-lang", "repo": "rust", "page": 2 },
  "headers": { "x-ratelimit-remaining": "4999" }
}
```

- `body` is parsed JSON, the text of a non-JSON body, or `null`.
- `next` is the argument object for the next page, read from a `Link: <…>; rel="next"` header. It is `null` on the last page, and also when the link pages with a query parameter the operation does not declare.
- `headers` keeps rate-limit headers, `retry-after`, and total-count headers.

A non-2xx status fails the call for the model with a one-line summary and the first 1 KiB of the body, and `data` is still returned. Transport errors and timeouts fail the call. Rho never retries.

### Limits

| Limit | Value | Behavior |
| --- | --- | --- |
| `max_response_bytes` | 4 MiB | A larger body fails the call, naming the limit and the actual size. GitHub's largest list pages at `per_page=100` measure about 1.9 MB. |
| Codemode heap | 8 MiB | A parsed body takes 2.4x to 3x its size in a script's heap, so one script holds about 2.7 MB of response bodies in total. Use a smaller `per_page`, or one script per page, for larger results. |
| `max_output_bytes` | [config](/configuration#tool-output-limit) | Truncates the text the model reads. Script `data` is not truncated. |
| Redirects | 10 | More fails the call. |
