use pretty_assertions::assert_eq;

use super::*;

fn mcp_config(servers: &str) -> McpConfig {
    toml::from_str(servers).expect("an [mcp] table")
}

// Covers: `rho/mcp/list` names each server's source, lists a server whose
// configuration is invalid with its error instead of failing the listing,
// and orders the config's and the plugins' servers by identity as `rho mcp
// list` does.
// Owner: MCP configuration listing.
#[test]
fn listing_names_each_servers_source_and_keeps_invalid_ones() {
    let config = mcp_config(
        r#"
        [servers.zeta]
        transport = "streamable_http"
        url = "https://example.com/mcp"
        enabled = false

        [servers.alpha]
        transport = "stdio"
        command = "fs-server"
        args = ["--root", "/work"]

        [servers.broken]
        transport = "stdio"
        "#,
    );
    let mut plugins = McpConfig::default();
    for (identity, server) in mcp_config(
        r#"
        [servers.search]
        transport = "stdio"
        command = "search"
        "#,
    )
    .servers
    {
        plugins.servers.insert(format!("demo/{identity}"), server);
    }

    let listing = ConfiguredMcp { config, plugins }.listing();

    let broken_error = listing
        .iter()
        .find(|server| server.identity == "broken")
        .and_then(|server| server.error.clone())
        .expect("the invalid server is listed with its error");
    assert_eq!(
        listing,
        vec![
            ConfiguredMcpServer::new(
                "alpha".into(),
                McpServerSource::Config,
                /*enabled*/ true,
                Some(McpTransport::Stdio {
                    command: "fs-server".into(),
                    args: vec!["--root".into(), "/work".into()],
                }),
                None,
            ),
            ConfiguredMcpServer::new(
                "broken".into(),
                McpServerSource::Config,
                /*enabled*/ false,
                None,
                Some(broken_error),
            ),
            ConfiguredMcpServer::new(
                "demo/search".into(),
                McpServerSource::Plugin {
                    plugin: "demo".into(),
                },
                /*enabled*/ true,
                Some(McpTransport::Stdio {
                    command: "search".into(),
                    args: Vec::new(),
                }),
                None,
            ),
            ConfiguredMcpServer::new(
                "zeta".into(),
                McpServerSource::Config,
                /*enabled*/ false,
                Some(McpTransport::StreamableHttp {
                    url: "https://example.com/mcp".into(),
                }),
                None,
            ),
        ]
    );
}
