//! ACP v2 error shapes that Rho's v1 adapter doesn't produce.

use agent_client_protocol::{
    v2::{schema as v2, Responder},
    Error as AcpError, ErrorCode, JsonRpcResponse,
};

/// `responder` for a request that names session `session_id`. v1 writes a
/// missing session's `-32002` with a `uri`, or with the daemon's text. v2
/// requires `data.sessionId` naming the requested session, so this
/// responder rewrites each `-32002` it answers with. That holds because the
/// adapter sends `-32002` only for sessions (`acp::resource_not_found`).
pub(super) fn session_scoped<T: JsonRpcResponse>(
    responder: Responder<T>,
    session_id: &v2::SessionId,
) -> Responder<T> {
    let session_id = session_id.clone();
    responder.wrap_params(move |_method, result| {
        result.map_err(|error| session_not_found(error, session_id))
    })
}

fn session_not_found(error: AcpError, session_id: v2::SessionId) -> AcpError {
    if error.code == ErrorCode::RESOURCE_NOT_FOUND {
        v2::Error::session_not_found(session_id).into()
    } else {
        error
    }
}
