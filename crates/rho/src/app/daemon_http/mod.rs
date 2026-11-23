//! `rho daemon serve --http`: exposes the local Rho daemon's `rho-protocol`
//! JSON-RPC stream over HTTP WebSockets (`GET /ws`), so clients on hosts that
//! cannot use SSH can still speak the full daemon protocol.
//!
//! Each WebSocket text frame carries one JSON-RPC 2.0 message (up to
//! [`rho_protocol::MAX_LINE_BYTES`]), relayed directly to the local daemon's
//! private socket so the remote client performs `rho/hello` end-to-end with
//! the daemon. For methods that take a `ClientEnvironment`, the bridge
//! overlays the client's variables on top of the server host's environment so
//! remote clients on another OS do not need to supply the host's `PATH`.
//!
//! Security guards match `rho mcp serve --http`:
//! - binds only loopback or a private/overlay network address;
//! - validates `Host` (loopback, bound private IP, `--allow-host`) and refuses
//!   any `Origin` not listed in `--allow-origin`;
//! - authenticates every upgrade with a bearer token from `rho daemon token`
//!   (stored hashed in a dedicated credential-store account, read fresh per
//!   upgrade so rotation/revocation applies immediately).

mod bridge;
mod server;
pub(super) mod tokens;

pub(super) use server::{serve_http, HttpOptions};
