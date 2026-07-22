use super::*;

#[tokio::test]
async fn callback_url_uses_the_bound_ipv4_target() {
    let listener = bind_ipv4(0).await.unwrap();
    let url = callback_url(&listener, "/callback").unwrap();
    assert_eq!(
        url,
        format!(
            "http://127.0.0.1:{}/callback",
            listener.local_addr().unwrap().port()
        )
    );
}

#[tokio::test]
async fn generated_callback_target_reaches_the_listener() {
    let listener = bind_ipv4(0).await.unwrap();
    let callback = url::Url::parse(&callback_url(&listener, "/callback").unwrap()).unwrap();
    let connection = TcpStream::connect((callback.host_str().unwrap(), callback.port().unwrap()))
        .await
        .unwrap();
    let (accepted, _) = listener.accept().await.unwrap();

    assert_eq!(
        connection.peer_addr().unwrap(),
        accepted.local_addr().unwrap()
    );
}

#[test]
fn socket_address_format_supports_ipv6_callback_targets() {
    let address: std::net::SocketAddr = "[::1]:1234".parse().unwrap();
    assert_eq!(
        format!("http://{address}/callback"),
        "http://[::1]:1234/callback"
    );
}
