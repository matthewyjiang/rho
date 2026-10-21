use super::*;

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
