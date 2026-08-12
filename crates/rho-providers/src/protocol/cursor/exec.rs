use super::connect::encode_client_message;
use super::proto::{
    agent_client_message, exec_client_message, exec_server_message, kv_client_message,
    AgentClientMessage, BackgroundShellSpawnResult, ClientHeartbeat, DeleteResult,
    DiagnosticsResult, ExecClientMessage, ExecServerMessage, FetchError, FetchResult,
    GetBlobResult, GrepError, GrepResult, KvClientMessage, LsResult, McpToolDefinition,
    PathRejected, ReadResult, RequestContext, RequestContextResult, RequestContextSuccess,
    SetBlobResult, ShellRejected, ShellResult, WriteResult, WriteShellStdinError,
    WriteShellStdinResult,
};

const NATIVE_REJECT_REASON: &str =
    "Tool not available in this environment. Use the MCP tools provided instead.";

pub(crate) fn heartbeat_frame() -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::ClientHeartbeat(
            ClientHeartbeat {},
        )),
    })
}

pub(crate) fn kv_get_blob_response(id: u32, blob_data: Option<Vec<u8>>) -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::KvClientMessage(
            KvClientMessage {
                id,
                message: Some(kv_client_message::Message::GetBlobResult(GetBlobResult {
                    blob_data,
                })),
            },
        )),
    })
}

pub(crate) fn kv_set_blob_response(id: u32) -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::KvClientMessage(
            KvClientMessage {
                id,
                message: Some(kv_client_message::Message::SetBlobResult(SetBlobResult {})),
            },
        )),
    })
}

pub(crate) fn request_context_success(
    exec: &ExecServerMessage,
    tools: Vec<McpToolDefinition>,
    cloud_rule: Option<String>,
) -> Vec<u8> {
    encode_exec_result(
        exec,
        exec_client_message::Message::RequestContextResult(RequestContextResult {
            result: Some(super::proto::request_context_result::Result::Success(
                RequestContextSuccess {
                    request_context: Some(RequestContext { tools, cloud_rule }),
                },
            )),
        }),
    )
}

pub(crate) fn native_exec_reject(exec: &ExecServerMessage) -> Option<Vec<u8>> {
    let result = match exec.message.as_ref()? {
        exec_server_message::Message::ReadArgs(args) => {
            exec_client_message::Message::ReadResult(ReadResult {
                result: Some(super::proto::read_result::Result::Rejected(path_rejected(
                    &args.path,
                ))),
            })
        }
        exec_server_message::Message::LsArgs(args) => {
            exec_client_message::Message::LsResult(LsResult {
                result: Some(super::proto::ls_result::Result::Rejected(path_rejected(
                    &args.path,
                ))),
            })
        }
        exec_server_message::Message::GrepArgs(_) => {
            exec_client_message::Message::GrepResult(GrepResult {
                result: Some(super::proto::grep_result::Result::Error(GrepError {
                    error: NATIVE_REJECT_REASON.into(),
                })),
            })
        }
        exec_server_message::Message::WriteArgs(args) => {
            exec_client_message::Message::WriteResult(WriteResult {
                result: Some(super::proto::write_result::Result::Rejected(path_rejected(
                    &args.path,
                ))),
            })
        }
        exec_server_message::Message::DeleteArgs(args) => {
            exec_client_message::Message::DeleteResult(DeleteResult {
                result: Some(super::proto::delete_result::Result::Rejected(
                    path_rejected(&args.path),
                )),
            })
        }
        exec_server_message::Message::ShellArgs(args)
        | exec_server_message::Message::ShellStreamArgs(args) => {
            exec_client_message::Message::ShellResult(shell_rejected(
                args.command.clone(),
                args.working_directory.clone(),
            ))
        }
        exec_server_message::Message::BackgroundShellSpawnArgs(args) => {
            exec_client_message::Message::BackgroundShellSpawnResult(BackgroundShellSpawnResult {
                result: Some(
                    super::proto::background_shell_spawn_result::Result::Rejected(ShellRejected {
                        command: args.command.clone(),
                        working_directory: args.working_directory.clone(),
                        reason: NATIVE_REJECT_REASON.into(),
                        is_readonly: false,
                    }),
                ),
            })
        }
        exec_server_message::Message::FetchArgs(args) => {
            exec_client_message::Message::FetchResult(FetchResult {
                result: Some(super::proto::fetch_result::Result::Error(FetchError {
                    url: args.url.clone(),
                    error: NATIVE_REJECT_REASON.into(),
                })),
            })
        }
        exec_server_message::Message::WriteShellStdinArgs(_) => {
            exec_client_message::Message::WriteShellStdinResult(WriteShellStdinResult {
                result: Some(super::proto::write_shell_stdin_result::Result::Error(
                    WriteShellStdinError {
                        error: NATIVE_REJECT_REASON.into(),
                    },
                )),
            })
        }
        exec_server_message::Message::DiagnosticsArgs(_) => {
            exec_client_message::Message::DiagnosticsResult(DiagnosticsResult {})
        }
        exec_server_message::Message::RequestContextArgs(_)
        | exec_server_message::Message::McpArgs(_) => return None,
    };
    Some(encode_exec_result(exec, result))
}

fn encode_exec_result(exec: &ExecServerMessage, message: exec_client_message::Message) -> Vec<u8> {
    encode_client_message(&AgentClientMessage {
        message: Some(agent_client_message::Message::ExecClientMessage(
            ExecClientMessage {
                id: exec.id,
                exec_id: exec.exec_id.clone(),
                message: Some(message),
            },
        )),
    })
}

fn path_rejected(path: &str) -> PathRejected {
    PathRejected {
        path: path.to_string(),
        reason: NATIVE_REJECT_REASON.into(),
    }
}

fn shell_rejected(command: String, working_directory: String) -> ShellResult {
    ShellResult {
        result: Some(super::proto::shell_result::Result::Rejected(
            ShellRejected {
                command,
                working_directory,
                reason: NATIVE_REJECT_REASON.into(),
                is_readonly: false,
            },
        )),
    }
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;
