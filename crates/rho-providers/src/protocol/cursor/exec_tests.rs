use crate::protocol::cursor::proto::{
    exec_server_message, ExecServerMessage, ReadArgs, RequestContextArgs,
};

use super::native_exec_reject;

// Covers: native Cursor tools must be rejected so the model falls back to Rho MCP tools
// Owner: cursor protocol
#[test]
fn native_exec_is_rejected_and_request_context_is_left_to_the_runtime() {
    let read = ExecServerMessage {
        id: 1,
        exec_id: "exec-1".into(),
        message: Some(exec_server_message::Message::ReadArgs(ReadArgs {
            path: "src/lib.rs".into(),
        })),
    };
    let context = ExecServerMessage {
        id: 2,
        exec_id: "exec-2".into(),
        message: Some(exec_server_message::Message::RequestContextArgs(
            RequestContextArgs {},
        )),
    };

    assert!(native_exec_reject(&read).is_some());
    assert!(native_exec_reject(&context).is_none());
}
