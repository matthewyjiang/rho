# rho-protocol

The versioned JSON-RPC protocol between the [Rho](https://github.com/matthewyjiang/rho)
daemon and its frontends: the Rho TUI, rho-ui, the `rho acp` adapter, and MCP.

- Newline-delimited JSON-RPC 2.0 over any `AsyncRead`/`AsyncWrite` stream.
- Every connection starts with `rho/hello`. Peers are compatible when their
  protocol majors match; a mismatch returns a typed error that says whether to
  restart the daemon or update the client.
- After the handshake, the daemon serves a connection's requests concurrently
  and answers each one when it completes, so responses can arrive out of
  order and pair with their requests by id. Requests also run concurrently, so
  a client sends a request that depends on another only after that one's
  response arrives. When the client stops sending or the daemon shuts down,
  the daemon answers every request it has already read, then closes the
  connection.
- `serve_connection` implements the daemon side around a `Service` with one
  async method per protocol method.
- The [`methods`](https://docs.rs/rho-protocol/latest/rho_protocol/methods/)
  module lists every method with its params and result. A thread's turns run
  in the daemon and outlive the client that started them.
- No dependency on `rho-sdk`, so an SDK major never breaks a frontend build.

```rust
use rho_protocol::{methods, Client, ClientError, ClientInfo, StatusParams};
use tokio::net::UnixStream;

async fn print_status(stream: UnixStream) -> Result<(), ClientError> {
    let client_info = ClientInfo {
        name: "my-frontend".into(),
        version: "0.1.0".into(),
    };
    let (mut client, hello) = Client::handshake(stream, client_info).await?;
    println!("connected to rho {}", hello.daemon.version);
    let status = client.request::<methods::Status>(&StatusParams {}).await?;
    println!("daemon pid {}", status.pid);
    Ok(())
}
```

The daemon socket location, permissions, and lifecycle are documented in the
[daemon guide](https://matthewyjiang.github.io/rho/daemon).
