use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::{error_for_status, MAX_ERROR_BODY_BYTES};
use crate::model::ModelError;

#[tokio::test]
async fn captures_and_truncates_error_response_bodies() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let body = "x".repeat(MAX_ERROR_BODY_BYTES + 100);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });

    let response = reqwest::get(format!("http://{address}")).await.unwrap();
    let error = error_for_status(response).await.unwrap_err();
    server.await.unwrap();

    let ModelError::HttpStatus { status, body } = error else {
        panic!("expected HTTP status error");
    };
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(body.matches('x').count(), MAX_ERROR_BODY_BYTES);
    assert!(body.ends_with("[response body truncated]"));
}
