//! How the server modules read a request's params and write its result:
//! typed by [`Method`], so the server stays on the types the client sends.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{methods::Method, ErrorCode, RpcError};

/// Decodes `M`'s params. Absent params decode like `{}`, so params-less
/// methods accept both. Typing by [`Method`] keeps the server on the same
/// params type the client sends.
pub(crate) fn decode_params<M: Method>(params: Option<Value>) -> Result<M::Params, RpcError> {
    serde_json::from_value(params.unwrap_or_else(|| Value::Object(Map::new())))
        .map_err(|error| RpcError::new(ErrorCode::InvalidParams, error.to_string()))
}

/// Encodes `M`'s result, the type [`crate::Client::request`] decodes.
pub(crate) fn encode_result<M: Method>(result: &M::Result) -> Result<Value, RpcError> {
    encode(result)
}

/// What a [`crate::Service`] or `SettingsService` method that is not overridden
/// answers: this daemon does not serve `method`.
pub(crate) fn unsupported<T>(method: &str) -> std::future::Ready<Result<T, RpcError>> {
    std::future::ready(Err(RpcError::new(
        ErrorCode::MethodNotFound,
        format!("this rho daemon does not serve {method}"),
    )))
}

/// Fails only for values JSON cannot hold, such as a non-UTF-8 path.
pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(ErrorCode::InternalError, error.to_string()))
}
