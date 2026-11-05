//! Frontend side of a connection: handshake, then typed requests.

use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};

use crate::{
    methods::{Hello, Method},
    ClientInfo, ErrorCode, FrameError, HelloParams, HelloResponse, Message, MessageReader,
    MessageWriter, ParseError, Request, RequestId, RpcError, VersionMismatch, PROTOCOL_VERSION,
};

/// A handshaken connection to the daemon.
///
/// Requests run one at a time: [`Client::request`] writes a request and reads
/// until its response. Protocol 1.0 defines no daemon notifications, so any
/// that arrive are skipped.
pub struct Client<S> {
    reader: MessageReader<ReadHalf<S>>,
    writer: MessageWriter<WriteHalf<S>>,
    next_id: i64,
}

/// Why a request or handshake failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("rho daemon closed the connection")]
    Closed,
    #[error("rho daemon sent an invalid message: {0}")]
    InvalidMessage(#[from] ParseError),
    #[error("rho daemon sent an unexpected message while {method} was pending")]
    UnexpectedMessage { method: &'static str },
    /// The daemon and this client speak incompatible protocol majors. The
    /// message says which side to change.
    #[error("{0}")]
    VersionMismatch(VersionMismatch),
    #[error("rho daemon rejected {method}: {error}")]
    Rpc {
        method: &'static str,
        error: RpcError,
    },
    #[error("could not encode {method} params: {source}")]
    EncodeParams {
        method: &'static str,
        source: serde_json::Error,
    },
    #[error("rho daemon sent a malformed {method} result: {source}")]
    DecodeResult {
        method: &'static str,
        source: serde_json::Error,
    },
}

impl<S: AsyncRead + AsyncWrite> Client<S> {
    /// Sends `rho/hello` and returns the connection once the daemon accepts
    /// this client's protocol.
    ///
    /// An incompatible daemon yields [`ClientError::VersionMismatch`], whether
    /// the daemon rejects the hello or reports an incompatible version.
    pub async fn handshake(
        stream: S,
        client: ClientInfo,
    ) -> Result<(Self, HelloResponse), ClientError> {
        let (reader, writer) = tokio::io::split(stream);
        let mut this = Self {
            reader: MessageReader::new(reader),
            writer: MessageWriter::new(writer),
            next_id: 1,
        };
        let params = HelloParams {
            protocol: PROTOCOL_VERSION,
            client,
        };
        let hello = match this.request::<Hello>(&params).await {
            Ok(hello) => hello,
            Err(ClientError::Rpc { method, error })
                if error.error_code() == Some(ErrorCode::VersionMismatch) =>
            {
                let mismatch = error
                    .data
                    .clone()
                    .and_then(|data| serde_json::from_value::<VersionMismatch>(data).ok());
                return Err(match mismatch {
                    Some(mismatch) => ClientError::VersionMismatch(mismatch),
                    None => ClientError::Rpc { method, error },
                });
            }
            Err(error) => return Err(error),
        };
        if let Some(mismatch) = VersionMismatch::between(hello.protocol, PROTOCOL_VERSION) {
            return Err(ClientError::VersionMismatch(mismatch));
        }
        Ok((this, hello))
    }

    /// Sends one request and waits for its response.
    pub async fn request<M: Method>(
        &mut self,
        params: &M::Params,
    ) -> Result<M::Result, ClientError> {
        let method = M::NAME;
        let params = serde_json::to_value(params)
            .map_err(|source| ClientError::EncodeParams { method, source })?;
        let id = RequestId::Number(self.next_id);
        self.next_id += 1;
        self.writer
            .write(&Message::Request(Request {
                id: id.clone(),
                method: method.to_owned(),
                params: Some(params),
            }))
            .await?;
        loop {
            let Some(line) = self.reader.read_line().await? else {
                return Err(ClientError::Closed);
            };
            match Message::parse(&line)? {
                Message::Notification(_) => continue,
                // A `null` id is the daemon rejecting a line it could not
                // read; with one request in flight, that line was ours.
                Message::Response(response)
                    if response.id.is_none() || response.id == Some(id.clone()) =>
                {
                    let result = response
                        .outcome
                        .map_err(|error| ClientError::Rpc { method, error })?;
                    return serde_json::from_value(result)
                        .map_err(|source| ClientError::DecodeResult { method, source });
                }
                Message::Response(_) | Message::Request(_) => {
                    return Err(ClientError::UnexpectedMessage { method })
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
