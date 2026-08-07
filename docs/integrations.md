# Integrations

Rho ships host and shell integrations in the binary. You do not install a plugin store or run init hooks for the pieces below.

| Integration | Role | Page |
| --- | --- | --- |
| Herdr | Agent state, sibling-pane attach, host image paste under a Herdr workspace | [Herdr](/integrations/herdr) |
| RTK | Rewrites agent shell commands when the `rtk` binary is on `PATH` | [RTK](/integrations/rtk) |
| MCP | Native stdio and Streamable HTTP tools | [Model Context Protocol](/integrations/mcp) |

Both stay optional. Outside Herdr, Rho skips host reporting. Without RTK, shell tools run commands unchanged.

## Choose a page

- [Herdr](/integrations/herdr) - detection env, state reporting, subagent panes, graphics
- [RTK](/integrations/rtk) - install, rewrite flow, config, analytics
- [Model Context Protocol](/integrations/mcp) - server config, transports, permissions, and lifecycle

Check either from the interactive TUI with `/doctor`.
