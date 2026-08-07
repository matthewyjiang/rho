# Integrations

Rho ships host and shell integrations in the binary. You do not install a plugin store or run init hooks for the pieces below.

| Integration | Role | Page |
| --- | --- | --- |
| Herdr | Agent state, sibling-pane attach, host image paste under a Herdr workspace | [Herdr](/integrations/herdr) |
| RTK | Rewrites agent shell commands when the `rtk` binary is on `PATH` | [RTK](/integrations/rtk) |
| MCP | Native stdio and Streamable HTTP tools | [Model Context Protocol](/integrations/mcp) |
| Agent Plugins | Local plugin packages contributing skills and MCP servers | [Agent Plugins](/integrations/plugins) |

All stay optional. Outside Herdr, Rho skips host reporting. Without RTK, shell tools run commands unchanged. MCP stays inert until you add an enabled server under `[mcp.servers]` or a plugin package provides one. Agent Plugins load only from the explicit plugin roots. Disabled packages stay visible in `rho plugins list` but do not contribute components.

## Choose a page

- [Herdr](/integrations/herdr) - detection env, state reporting, subagent panes, graphics
- [RTK](/integrations/rtk) - install, rewrite flow, config, analytics
- [Model Context Protocol](/integrations/mcp) - server config, transports, permissions, lifecycle, `/mcp`, and `rho mcp`
- [Agent Plugins](/integrations/plugins) - package layout, discovery roots, install and activation, skills and MCP components, failure isolation

Check setup from the interactive TUI with `/doctor`. Inspect MCP with `/mcp` or `rho mcp list`. Manage packages with `rho plugins`. Plugin load problems appear in doctor and plugin inventory output.
