# rho-protocol

The versioned JSON-RPC protocol between the [Rho](https://github.com/matthewyjiang/rho)
daemon and its frontends: the Rho TUI, rho-ui, the `rho acp` adapter, and MCP.

- Newline-delimited JSON-RPC 2.0 over any `AsyncRead`/`AsyncWrite` stream.
- Every connection starts with `rho/hello`. The protocol is unstable while
  its major is 0: peers are compatible only when their protocol versions are
  equal. From major 1 on, they are when their majors match. A mismatch
  returns a typed error that says whether to restart the daemon or update the
  client, and what a daemon restart would cost: how much work in progress it
  interrupts and how many other clients it disconnects.
- After the handshake, the daemon serves a connection's requests concurrently
  and answers each one when it completes, so responses can arrive out of
  order and pair with their requests by id. Requests also run concurrently, so
  a client sends a request that depends on another only after that one's
  response arrives. When the client stops sending or the daemon shuts down,
  the daemon answers every request it has already read, then closes the
  connection.
- A client that attaches to a session receives its events as
  `rho/session/event` notifications, never before the attach response, until
  it detaches. A client that falls a whole backlog behind gets
  `rho/session/detached` and reattaches with its cursor; a turn never waits
  on a client. As a connection closes, the events already published to it,
  and the reason of a publisher that already closed, still reach the client.
- `serve_connection` implements the daemon side around a `Service` with one
  async method per protocol method, fed by one `subscription` per attached
  session. A method the service does not override answers
  `MethodNotFound`, so a partial daemon, such as a test double, implements
  only what it serves. `Client` implements the frontend side: overlapping
  requests, and the daemon's notifications on a separate `Notifications`
  stream.
- `DaemonEndpoint::locate` says where a rho home's daemon listens, the same
  layout the `rho` CLI uses. On Unix, `LocalDaemon` connects a frontend to
  that daemon and, when none runs, starts it with `rho daemon start`, the
  CLI's own ensure-running path.
- The [`methods`](https://docs.rs/rho-protocol/latest/rho_protocol/methods/)
  module lists every method and notification with its params and result. A
  session's turns run in the daemon and outlive the client that started them.
- `schema/protocol.json` is the protocol as JSON Schema, for clients in other
  languages: every method, notification, and error code. CI keeps it current
  and compatible with the last release's.
- No dependency on `rho-sdk`, so an SDK major never breaks a frontend build.

```rust
use rho_protocol::{methods, Client, ClientError, ClientHello, ClientInfo, StatusParams};
use tokio::net::UnixStream;

async fn print_status(stream: UnixStream) -> Result<(), ClientError> {
    // A frontend answers its sessions' approvals and questions; a command
    // that answers none says `ClientHello::command`.
    let me = ClientHello::frontend(ClientInfo {
        name: "my-frontend".into(),
        version: "0.1.0".into(),
    });
    let (client, _notifications, hello) = Client::handshake(stream, me).await?;
    println!("connected to rho {}", hello.daemon.version);
    let status = client.request::<methods::Status>(&StatusParams {}).await?;
    println!("daemon pid {}", status.pid);
    Ok(())
}
```

The wire protocol is documented in the
[protocol reference](https://matthewyjiang.github.io/rho/protocol), and the
daemon socket location, permissions, and lifecycle in the
[daemon guide](https://matthewyjiang.github.io/rho/daemon).
