# Agent Plugins

Rho loads [Agent Plugins](https://agent-plugins.org/specification) 1.0.0
packages from explicit local roots. A plugin is a directory with a
`plugin.json` manifest. It contributes skills, MCP servers, Rho agent
definitions, or any mix. Plugin skills join the ordinary metadata-first skill
flow. Plugin MCP servers run on the same native runtime as ordinary
`[mcp.servers]` entries. Plugin agents join the agent catalog.

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
├── mcp.json
└── io.github.matthewyjiang.rho/
    └── agents/
        └── auditor.md
```

`plugin.json` is required and loads before any component. `skills/`,
`mcp.json`, and the Rho extension directory are optional; a missing location
is not an error.

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

Only immediate child directories, and immediate child symlinks whose resolved
targets contain a `plugin.json`, are plugins. When two roots contain the same
plugin name, the nearer root wins and Rho reports the shadowed copy. Installed
and linked packages use these same roots.

Outside a repository the project root is the current directory, so running
from your home directory (or inside a repository rooted there, such as a
dotfiles repository) makes the project root your home directory and its
plugins root `~/.agents/plugins` itself. Rho then treats that folder as user
scope only and lists each package once.

Supported component types in the current build: skills, and MCP servers with
the `stdio` and `streamable-http` transports. Legacy `sse` entries are
skipped per server. Rho also implements one client extension namespace,
`io.github.matthewyjiang.rho`, for [agents](#agents).

## Project plugin trust

Packages under `~/.agents/plugins` are your own files and always activate.
Packages under a project's `.agents/plugins` activate only in trusted
workspaces:

```sh
RHO_TRUST_PROJECT_PLUGINS=1 rho
```

This is the same family as `RHO_TRUST_PROJECT_HOOKS` and
`RHO_TRUST_PROJECT_AGENTS`, so a cloned repository cannot silently run plugin
commands or agents. Until you grant trust, Rho inventories project plugins.
Names, manifests, skill names, agent file names, and MCP server definitions stay
visible in
`rho plugins list` and `rho plugins inspect`, but no component activates:
plugin skills do not join skill discovery, plugin agents do not join the agent
catalog, and no MCP server, including stdio commands, starts. An untrusted
project plugin does not shadow a user plugin
with the same name; enabling or disabling it still records your policy, and
`rho plugins enable` says when trust is the remaining gate.

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

## Agents

Agent Plugins 1.0.0 standardizes only skills and MCP servers; agents are
outside the portable format. Rho reads agent definitions from its own client
extension directory, which the specification reserves for client-specific
files and requires other clients to ignore, so a plugin that ships Rho agents
stays portable:

```text
<plugin>/io.github.matthewyjiang.rho/agents/*.md
```

Each file is an ordinary [agent definition](/subagents/definition-schema).
Agent files must be regular files: Rho skips a symlinked agent file and
reports it in `rho plugins inspect`.
Plugin agents appear in `/agents`, the `agent` tool, `--agent`, workflows, and
`/agent:<id>` in ACP sessions (including T3 Code delegation) like any other
agent. Their files stay read-only: `/agents` cannot edit or delete them. They
rank above built-ins and below every agent file you own:

```text
project .agents/agents > ~/.rho/agents > ~/.agents/agents > project plugins > user plugins > built-in
```

A plugin agent cannot use a built-in agent ID (`default`, `explorer`,
`reviewer`, `worker`), an internal agent ID, or an ID Rho uses elsewhere
(`side`, `permission-classifier-screen`). Rho skips such a file and
reports it; the built-in stays available. Two plugins at the same scope that
ship the same agent ID make that ID unavailable at that level and below
(project plugin duplicates also hide a user plugin agent with that ID) until
one is removed or disabled.

### Plugin agent models

You can still choose the model a Rho-runtime plugin agent runs, the same way
internal agents allow it. In `/agents`, select the plugin agent and press
Enter to open its model picker. **Use agent default** removes the override and
returns to the model the plugin's file names. The choice saves to config at
once and applies to the next launch of that agent:

```toml
[internal_agents.plugin-reviewer]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
```

The entry replaces the file's model and `fast` setting, so it must name a Rho
`provider` and `model` (a `@alias` whose target names a provider counts).
Config load drops an entry that leaves either out or names another
`runtime`, with a warning, and the file's model stays; the next config save
removes the dropped entry. The
optional `reasoning` key replaces the file's reasoning; without one the file's
reasoning stays. The entry is keyed by agent ID and applies to any Rho agent
with that ID, including an agent file of yours (for example one that takes
over a plugin agent's ID); `/agents` names the entry in that agent's detail
pane, and only plugin agents get the picker. Built-in, internal-only, and
reserved IDs never take one. Rho refuses to launch an agent that runs on another runtime while it has
an entry, since the entry would switch runtime rather than model, and
`/agents` marks such an entry as unable to apply. An explicit
[per-call `model`](/subagents#per-call-model-selection) on an `agent` call
replaces the entry's provider, model, and auth for that call; the entry's
`reasoning` and its fast mode (off) still apply.

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
for a fixed location, the skill for a `SKILL.md`, the agent for an agent file,
or the server entry for a command or working directory.

## Install roots and lifecycle

Rho manages packages in the same explicit roots it discovers:

```text
~/.agents/plugins/<name>/          user scope (default for install and link)
<repository>/.agents/plugins/<name>/   project scope (`--scope project`)
```

When the project plugins root is `~/.agents/plugins` (see
[Discovery roots](#discovery-roots)), `--scope project` fails with a pointer to
`--scope user`, and `rho plugins update` walks only user scope.

Lifecycle commands:

```bash
rho plugins list
rho plugins inspect <name>
rho plugins install <path-or-url> [--scope user|project] [--force]
rho plugins link <path> [--scope user|project] [--force]
rho plugins update [name]
rho plugins enable <name>
rho plugins disable <name>
rho plugins remove <name> [--yes]
```

Behavior:

- `install` validates `plugin.json` first, then copies the package into the
  managed root. It never executes package code. The source can be a local
  directory or a git repository; see [Installing from git](#installing-from-git).
- `link` validates first, then creates a directory symlink in the managed root.
- `update` refetches git-installed packages and replaces the ones whose ref
  moved to a new commit. Enable and disable state is kept.
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

### Installing from git

`rho plugins install` accepts GitHub links copied from the browser and other
git URLs:

```bash
# Repository root on the default branch
rho plugins install https://github.com/owner/repo

# A branch, tag, or commit, and a plugin directory inside the repository
rho plugins install https://github.com/owner/repo/tree/v1.2.0/plugins/demo

# Any other git remote: default branch, repository root
rho plugins install git@gitlab.com:owner/repo.git
```

Rho runs your `git` to shallow-fetch one commit into a temporary checkout,
so your existing credentials, SSH keys, and `insteadOf` rewrites apply to
private repositories. It then validates and copies the package like a local
install, without the `.git` directory. Accepted URL schemes are `https://`,
`http://`, `ssh://`, `git://`, `file://`, and scp-style `git@host:path`.
Arguments with a URL scheme (matched in any case) or a `git@` prefix are
always treated as remotes, and unsupported schemes are rejected. A scheme-less
argument such as `github.com/owner/repo` installs locally when that path
exists, and is treated as a GitHub link otherwise.
URLs with embedded credentials (`https://user:token@host/...`) are refused,
because the URL is recorded and printed; use a credential helper or SSH key.

In a GitHub `tree/` link, the first segment after `tree/` is the ref, and
percent-encoded segments are decoded. A branch whose name contains `/` must
be written encoded (`tree/feature%2Fx/...`), or use a tag or commit link.

The source URL, ref, package path, and installed commit are recorded in
`plugins.toml`, and `rho plugins inspect` shows them. `rho plugins update`
fetches the same ref again:

- A branch ref, or no ref (the remote default branch), moves to the newest
  commit.
- A tag or commit ref stays put unless the tag itself moves.
- An update is refused when the fetched package has a different `plugin.json`
  name than the installed one.

`rho plugins update` with no name updates every git-installed package in the
project and user roots, reports each result, and exits nonzero if any failed.
Packages installed from a local path have no source to refetch; reinstall
them with `--force`.
Project packages are updated only in a trusted workspace
(`RHO_TRUST_PROJECT_PLUGINS=1`); otherwise `update` reports them as skipped
without fetching.

### Activation state

Enable and disable state is Rho policy, not part of the Agent Plugins package
format. State files live outside package directories:

```text
$RHO_HOME/plugins.toml          user scope (default `~/.rho/plugins.toml`)
<repository>/.rho/plugins.toml  project scope
```

When the project root is your home directory, there is no project scope. With
a separate `RHO_HOME`, older Rho builds recorded `disable` from there in
`~/.rho/plugins.toml`; those disables still apply to the matching user plugin
until you run `rho plugins enable` or `rho plugins remove` for it.

A missing file means every discovered plugin is enabled. Disabling writes
`enabled = false` for that package name in the matching scope file. Package
contents are never modified. Git installs also record their source here:

```toml
[plugins.demo]
origin = "git"

[plugins.demo.git]
url = "https://github.com/owner/repo"
ref = "main"
subdir = "plugins/demo"
commit = "53bd86b517a24260cb2c42d09e1e86489bdecf70"
```

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
  output paths as ordinary MCP configuration. Git installs and updates only
  fetch and copy files; git runs with hooks and templates disabled, so
  user-level `post-checkout` hooks never act on fetched content. Review a
  repository before installing it, because its skills and MCP servers become
  active in new sessions. Package registries, automatic background updates,
  and package signatures are not implemented yet.
- Git sources recorded in a project `plugins.toml` arrive with cloned
  repositories. `rho plugins update` fetches them only in a trusted
  workspace, and checks every recorded source against the same URL scheme,
  credential, and path rules before fetching.

## Relationship to ordinary configuration

Ordinary `[mcp.servers]` entries and loose skills keep working unchanged.
Plugin servers merge with ordinary servers under their namespaced identities,
and ordinary config keeps full precedence for its own identities.

What comes from Agent Plugins versus Rho policy:

| Behavior | Source |
| --- | --- |
| Package layout, `plugin.json`, skills/, `mcp.json`, path containment | Agent Plugins |
| Explicit discovery roots and component loading | Agent Plugins, with Rho root choices |
| Install, link, update, enable, disable, remove commands | Rho policy |
| Enablement state files and managed install roots | Rho policy |
| Skill and MCP runtime behavior after load | Rho |
| `io.github.matthewyjiang.rho/agents` extension directory | Rho client extension |

Package registries and automatic background updates remain unsupported. Git
installs update only when you run `rho plugins update`.
