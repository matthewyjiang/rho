use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

const BAD_KEY: &str = "sk-bad-<key>";
const GOOD_KEY: &str = "sk-good";

fn labels() -> KeyEntryLabels {
    KeyEntryLabels::new("Moonshot AI", "Moonshot API key")
}

/// Sends one request to the page and returns its status and body.
async fn send(port: u16, request: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("an HTTP status line: {response:?}"));
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

fn get(path: &str, host: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")
}

fn post(path: &str, host: &str, key: &str) -> String {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("key", key)
        .finish();
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// The path the page's form posts to.
fn form_action(body: &str) -> String {
    let start = body.find(r#"action=""#).expect("a form") + r#"action=""#.len();
    body[start..][..body[start..].find('"').unwrap()].to_owned()
}

// Covers: the URL token opens the page once and any reuse is refused; a
// request naming another Host (DNS rebinding) is refused without opening the
// page; a rejected key is never echoed and its reason is escaped; only the
// form token the page issued takes a key; an accepted key ends the page.
// Owner: key entry page security contract.
#[tokio::test]
async fn the_page_takes_one_key_from_its_own_origin() {
    let page = KeyEntryPage::bind(labels()).await.unwrap();
    let url = url::Url::parse(page.url()).unwrap();
    let port = url.port().unwrap();
    let host = format!("127.0.0.1:{port}");
    let open = url.path().to_owned();
    let checked = Arc::new(Mutex::new(Vec::new()));
    let serving = tokio::spawn(page.serve(Duration::from_secs(60), {
        let checked = Arc::clone(&checked);
        move |key: EnteredKey| {
            let key = key.expose().to_owned();
            checked.lock().unwrap().push(key.clone());
            async move {
                if key == GOOD_KEY {
                    KeyVerdict::Accepted { notice: None }
                } else {
                    KeyVerdict::Rejected("<b>refused</b>".into())
                }
            }
        }
    }));

    let rebound = send(port, &get(&open, &format!("rebind.example:{port}"))).await;
    assert_eq!(rebound.0, 403, "another Host is refused");

    let (status, page) = send(port, &get(&open, &host)).await;
    assert_eq!(status, 200, "the first open shows the form: {page}");
    let form = form_action(&page);

    let reopened = send(port, &get(&open, &host)).await;
    assert_eq!(reopened.0, 410, "the URL token opens the page once");
    let guessed = send(port, &post(&open, &host, BAD_KEY)).await;
    assert_eq!(guessed.0, 404, "the URL token does not take keys");

    let blank = send(port, &post(&form, &host, "  ")).await;
    assert_eq!(blank.0, 200);
    let (status, rejected) = send(port, &post(&form, &host, BAD_KEY)).await;
    assert_eq!(status, 200);
    assert!(
        rejected.contains("&lt;b&gt;refused&lt;/b&gt;"),
        "the reason is shown escaped: {rejected}"
    );
    assert!(!rejected.contains("sk-bad"), "the key is never echoed");
    let foreign = send(port, &post(&form, &format!("localhost:{port}"), GOOD_KEY)).await;
    assert_eq!(foreign.0, 403, "a key from another Host is refused");

    let accepted = send(port, &post(&form, &host, &format!(" {GOOD_KEY}\n"))).await;
    assert_eq!(accepted.0, 200);
    tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the page stops once a key is accepted")
        .unwrap()
        .unwrap();
    assert_eq!(
        *checked.lock().unwrap(),
        vec![BAD_KEY.to_owned(), GOOD_KEY.to_owned()],
        "only non-blank keys from the issued form reach the check, trimmed"
    );
}

// Covers: an unused page expires and gives up its port.
// Owner: key entry page lifetime.
#[tokio::test(start_paused = true)]
async fn an_unused_page_expires_and_frees_its_port() {
    let page = KeyEntryPage::bind(labels()).await.unwrap();
    let port = url::Url::parse(page.url()).unwrap().port().unwrap();
    let after = Duration::from_secs(15 * 60);
    let result = page
        .serve(after, |_key| async {
            KeyVerdict::Accepted { notice: None }
        })
        .await;
    assert!(
        matches!(result, Err(KeyEntryError::Expired { after: expired }) if expired == after),
        "{result:?}"
    );
    TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("the expired page freed its port");
}
