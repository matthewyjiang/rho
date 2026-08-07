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
reports the shadowed copy. Installed and linked packages use these same roots.

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

## Install roots and lifecycle

Rho manages packages in the same explicit roots it discovers:

```text
~/.agents/plugins/<name>/          user scope (default for install and link)
<repository>/.agents/plugins/<name>/   project scope (`--scope project`)
```

Lifecycle commands:

```bash
rho plugins list
rho plugins inspect <name>
rho plugins install <path> [--scope user|project] [--force]
rho plugins link <path> [--scope user|project] [--force]
rho plugins enable <name>
rho plugins disable <name>
rho plugins remove <name> [--yes]
```

Behavior:

- `install` validates `plugin.json` first, then copies the package into the
  managed root. It never executes package code.
- `link` validates first, then creates a directory symlink in the managed root.
- `inspect` and `list` read manifests and component metadata only. They do not
  connect MCP servers or run skill scripts. Use `rho mcp list` / `/mcp` for live
  MCP status.
- `disable` keeps package files on disk and drops the package's skills and MCP
  servers from new sessions.
- `remove` deletes only the package directory or symlink under a managed root.
  It does not delete `<plugins-root>/data/<plugin>` runtime data.
- `--force` is required to replace an existing package at the destination.
- Destination paths must be immediate children of a managed root. The reserved
  `data/` directory is never a package slot.

### Activation state

Enable and disable state is Rho policy, not part of the Agent Plugins package
format. State files live outside package directories:

```text
$RHO_HOME/plugins.toml          user scope (default `~/.rho/plugins.toml`)
<repository>/.rho/plugins.toml  project scope
```

A missing file means every discovered plugin is enabled. Disabling writes
`enabled = false` for that package name in the matching scope file. Package
contents are never modified.

### Precedence

Discovery order and conflict rules:

```text
project plugins (nearest ancestor first) > user plugins
```

Within that order:

1. The first valid package for a given plugin name wins.
2. Later packages with the same name are reported as shadowed.
3. Disabled packages stay visible in `rho plugins list` / `inspect` but do not
   contribute skills or MCP servers.
4. Loose skills still outrank every plugin skill
   (`built-in > loose user > loose project > project plugins > user plugins`).
5. Ordinary `[mcp.servers]` identities stay separate from plugin-scoped
   `<plugin>/<server>` identities.

When the same plugin name exists in more than one root, enable, disable, and
remove act on the higher-precedence match.

## Diagnostics

- Session logs report rejected plugins, shadowed duplicates, disabled packages,
  invalid skills, invalid MCP entries, unsupported transports, and plugins with
  no usable components.
- `/doctor` includes an Agent Plugins check with loaded, disabled, and problem
  counts.
- `rho plugins list` and `rho plugins inspect` show package inventory, scope,
  origin (`local`, `install`, or `link`), enablement, component names, and
  diagnostics without executing package code.
- `/mcp` and `rho mcp list` show live plugin servers alongside ordinary servers,
  with status, errors, and exported tool names.
- Plugin-owned skills keep their owner in the skill source model
  (`plugin <name> (...)`), so inventory and skill listings can show which
  package contributed a skill.

## Trust

- Manifest inspection, install validation, and list/inspect never execute
  package code.
- Skills can still direct the agent to run scripts and touch resources through
  ordinary Rho permissions after a package is enabled and loaded into a session.
- MCP servers from enabled packages use the same permission, cancellation, and
  output paths as ordinary MCP configuration. Remote registry install, automatic
  updates, and package signatures are not implemented yet.

## Relationship to ordinary configuration

Ordinary `[mcp.servers]` entries and loose skills keep working unchanged.
Plugin servers merge with ordinary servers under their namespaced identities,
and ordinary config keeps full precedence for its own identities.

What comes from Agent Plugins versus Rho policy:

| Behavior | Source |
| --- | --- |
| Package layout, `plugin.json`, skills/, `mcp.json`, path containment | Agent Plugins |
| Explicit discovery roots and component loading | Agent Plugins, with Rho root choices |
| Install, link, enable, disable, remove commands | Rho policy |
| Enablement state files and managed install roots | Rho policy |
| Skill and MCP runtime behavior after load | Rho |

Remote registry distribution and automatic updates remain unsupported.
