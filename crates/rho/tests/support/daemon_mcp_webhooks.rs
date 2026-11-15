//! Webhook watches through the real binary: an HTTP host starts a session
//! with `notify`, and the server POSTs the session's news to a webhook the
//! owner added with `rho mcp webhook add`, with its bearer secret.

use std::{convert::Infallible, io::Write, process::Stdio};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::{bash, call, http_client, http_server, rotate, Sandbox, DEADLINE};

/// One POST the receiver got: its `Authorization` header and JSON body.
type Received = (Option<String>, Value);

/// A webhook receiver on a free loopback port that answers every POST 200
/// and hands it to the returned channel; returns its URL too.
async fn receiver() -> (String, mpsc::UnboundedReceiver<Received>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let (sender, received) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let sender = sender.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let sender = sender.clone();
                        async move {
                            let authorization = request
                                .headers()
                                .get(hyper::header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned);
                            let body = request.into_body().collect().await.unwrap().to_bytes();
                            let _ = sender
                                .send((authorization, serde_json::from_slice(&body).unwrap()));
                            Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::new())))
                        }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (url, received)
}

/// Adds webhook `name` at `url` for `token`, its secret piped on stdin.
fn add_webhook(sandbox: &Sandbox, name: &str, url: &str, token: &str, secret: &str) {
    let mut add = sandbox
        .rho("tool-call")
        .args([
            "mcp", "webhook", "add", name, "--url", url, "--token", token,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    add.stdin
        .take()
        .unwrap()
        .write_all(secret.as_bytes())
        .unwrap();
    let output = add.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

// Covers: `new` with `notify` watches the session it starts, and once the
// session settles the server POSTs the news, as `wait` would return it plus
// the server's `--name`, to the webhook's URL with its bearer secret.
// Owner: rho mcp serve --http webhook watches, through the actual binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notify_posts_the_settled_session_to_its_webhook() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = sandbox.workspace.display().to_string();
    let token = rotate(
        &sandbox,
        "bot",
        &["--scope", "write", "--workspace", &workspace],
    );
    let (hook, mut received) = receiver().await;
    add_webhook(&sandbox, "hook", &hook, "bot", "hook-secret\n");
    let (_server, url) = http_server(&sandbox, &["--name", "lab-box"], &[]).await;
    let host = http_client(&url, &token).await;

    let started = call(
        &host,
        "new",
        json!({"workspace": sandbox.workspace, "prompt": bash("echo done"), "notify": "hook"}),
    )
    .await
    .unwrap();
    let (authorization, body) = tokio::time::timeout(DEADLINE, received.recv())
        .await
        .expect("the server POSTs the news")
        .expect("the receiver runs");

    assert_eq!(
        (
            started["watching"].clone(),
            authorization,
            body["server"].clone(),
            body["session"].clone(),
            body["state"].clone(),
            body["done"].clone(),
            body["outcome"]["type"].clone(),
        ),
        (
            json!(true),
            Some("Bearer hook-secret".to_owned()),
            json!("lab-box"),
            started["session"].clone(),
            json!("completed"),
            json!(true),
            json!("answered"),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
