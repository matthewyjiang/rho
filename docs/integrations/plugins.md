# Agent Plugins

Rho loads [Agent Plugins](https://agent-plugins.org/specification) 1.0.0
packages from explicit local roots. A plugin is a directory with a
`plugin.json` manifest. It contributes skills, MCP servers, or both. Plugin
skills join the ordinary metadata-first skill flow. Plugin MCP servers run on
the same native runtime as ordinary `[mcp.servers]` entries.

The Agent Plugins 1.0.0 specification is a Working Draft. Rho selects the
validation contract from the manifest `$schema` identifier and never fetches
schemas at runtime.

## Package layout

```text
my-plugin/
├── plugin.json
├── skills/
│   └── summarize/
│       ├── SKILL.md
│       └── references/
│           └── checklist.md
└── mcp.json
```

`plugin.json` is required and loads before any component. `skills/` and
`mcp.json` are optional; a missing location is not an error.

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "Brief plugin description"
}
```

The manifest schema is closed. Rho reports and ignores unknown top-level
fields, and reports and ignores a non-object `extensions` field. Any other
schema violation rejects the plugin before component discovery. Plugin names
use 1-64 characters from `a-z`, `0-9`, `-`, and `.`, and start and end with a
letter or digit.

## Discovery roots

Rho checks explicit roots only. It never searches arbitrary directories.

```text
<project>/.agents/plugins/<plugin>/plugin.json   (nearest directory first, up to the repository root)
~/.agents/plugins/<plugin>/plugin.json
```

Only immediate child directories that contain a `plugin.json` are plugins.
When two roots contain the same plugin name, the nearer root wins and Rho
reports the shadowed copy.

Supported component types in the current build: skills, and MCP servers with
the `stdio` and `streamable-http` transports. Legacy `sse` entries are
skipped per server. Rho implements no client extension namespaces.

## Skills

Each immediate child directory of `skills/` with a regular `SKILL.md` is one
skill. Rho does not recurse for nested skills. Frontmatter follows the
[Agent Skills specification](https://agentskills.io/specification), and the
skill `name` must match its directory name. An invalid skill is skipped and
reported; valid siblings and the plugin's MCP servers still load.

Plugin skills sit below every loose skill location in
[skill precedence](/skills#where-rho-looks-for-skills):

```text
built-in > loose user > loose project > project plugins > user plugins
```

Duplicate skill names across sources keep the higher-precedence copy and log
the selected and ignored sources.

## MCP servers

`mcp.json` at the plugin root declares servers. Rho validates the top-level
document first, then each server independently, and translates valid entries
into the ordinary native MCP configuration. Transport setup, handshake, tool
discovery, namespacing, permissions, and shutdown all reuse the path
documented in [Model Context Protocol](/integrations/mcp). A plugin server
appears in inventories as `<plugin-name>/<server-name>`.

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "validator": {
      "type": "stdio",
      "command": "./bin/validator",
      "args": ["--data", "${PLUGIN_DATA}/validator"],
      "env": { "CONFIG": "${PLUGIN_ROOT}/config.json" },
      "cwd": "${PLUGIN_ROOT}"
    },
    "deployment-api": {
      "type": "streamable-http",
      "url": "https://deploy.example.com/mcp",
      "headers": { "X-Tenant": "public-tenant" }
    }
  }
}
```

Rules Rho enforces:

- `mcp.json` must target the same Agent Plugins version as `plugin.json`.
- A stdio `command` is one token: a bare executable name resolved through the
  platform search path, or a plugin-relative path starting with `./`.
- `cwd` defaults to the plugin root. An explicit `cwd` is plugin-relative,
  `${PLUGIN_ROOT}`-rooted, or `${PLUGIN_DATA}`-rooted, and must stay inside
  that directory.
- `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` expand in `args`, environment values,
  and `cwd` only. Expansion is single-pass and non-recursive. Unrecognized
  placeholder-like text stays literal. Placeholders never apply to `command`,
  URLs, headers, or environment variable names.
- Rho provides `PLUGIN_ROOT` and `PLUGIN_DATA` in each stdio subprocess
  environment. The data directory lives at `<plugins-root>/data/<plugin>` and
  persists across plugin updates. Plugins cannot override either variable.
- Remote URLs are absolute HTTP or HTTPS URLs without user information or
  fragments, and non-loopback endpoints must use HTTPS. Headers are literal
  package data. Rho disables HTTP redirects for these transports, so
  configured headers never cross origins.

An invalid top-level `mcp.json` disables MCP for that plugin only. An invalid
server entry disables only that entry. A plugin with no valid MCP servers
adds no MCP startup work.

## Containment and failure isolation

Every package-supplied path Rho reads or executes must resolve inside the
filesystem-resolved plugin root. Symlinks that escape the root are rejected
at the narrowest boundary: the plugin for `plugin.json`, the component type
for a fixed location, the skill for a `SKILL.md`, or the server entry for a
command or working directory.

## Diagnostics

- Session logs report rejected plugins, shadowed duplicates, invalid skills,
  invalid MCP entries, unsupported transports, and plugins with no usable
  components.
- `/doctor` includes an Agent Plugins check with loaded and problem counts.
- `/mcp` and `rho mcp list` show plugin servers alongside ordinary servers,
  with status, errors, and exported tool names.

## Relationship to ordinary configuration

Ordinary `[mcp.servers]` entries and loose skills keep working unchanged.
Plugin servers merge with ordinary servers under their namespaced identities,
and ordinary config keeps full precedence for its own identities. Plugin
loading adds no install, update, enable, or disable commands, and no remote
plugin downloads.
