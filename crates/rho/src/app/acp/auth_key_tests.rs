use std::{future::Future, pin::Pin, sync::Mutex};

use agent_client_protocol::schema::v1::{
    CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse,
    ElicitationAcceptAction, ElicitationAction, ElicitationMode, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification,
};
use pretty_assertions::assert_eq;
use rho_providers::{
    credentials::{CredentialResult, MemoryCredentialStore},
    provider::{self, ProviderAuthKind},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

use super::*;

const AUTH: &str = "moonshot-api-key";
const GOOD_KEY: &str = "sk-moon-good";
const BAD_KEY: &str = "sk-moon-bad";

/// Serves `GET /v1/models`: models for [`GOOD_KEY`], and `other` (a status
/// line) with the key quoted back for anything else.
async fn models_endpoint(other: &'static str) -> (Url, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/v1", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            let (status, body) = if request.contains(&format!("authorization: bearer {GOOD_KEY}")) {
                ("200 OK", r#"{"data":[{"id":"kimi-k2"}]}"#.to_owned())
            } else {
                (other, format!(r#"{{"error":"invalid key {BAD_KEY}"}}"#))
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (base, server)
}

/// A credential store that records every save.
#[derive(Default)]
struct RecordingStore {
    inner: MemoryCredentialStore,
    saved: Mutex<Vec<(String, String)>>,
}

impl CredentialStore for RecordingStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.inner.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.saved
            .lock()
            .unwrap()
            .push((account.into(), secret.into()));
        self.inner.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.inner.delete_secret(account)
    }
}

/// The pages a browser was served, as status and full response, in order.
type Pages = Vec<(u16, String)>;

/// What the user does once the host shows the key page's link.
#[derive(Clone, Copy, Debug)]
enum User {
    /// Opens the page and types a bad key, then a good one.
    EntersKeys,
    Declines,
    Dismisses,
    /// Never answers; the host leaves with `$/cancel_request`.
    Leaves,
}

/// Records every ACP message Rho sends, and plays the user's browser.
struct Host {
    user: User,
    shown: Mutex<Vec<CreateElicitationRequest>>,
    completed: Mutex<Vec<CompleteElicitationNotification>>,
    /// The pages the browser was served, in order.
    browser: Mutex<Option<JoinHandle<Pages>>>,
    showed: tokio::sync::Notify,
}

impl Host {
    fn new(user: User) -> Self {
        Self {
            user,
            shown: Mutex::default(),
            completed: Mutex::default(),
            browser: Mutex::default(),
            showed: tokio::sync::Notify::new(),
        }
    }
}

impl AcpClientPort for Host {
    fn send_session_notification(
        &self,
        _notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        unreachable!("sign-in sends no session updates")
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        unreachable!("sign-in asks no permission")
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        let ElicitationMode::Url(mode) = &request.mode else {
            panic!("the key page is a URL elicitation: {request:?}");
        };
        let url = mode.url.clone();
        self.shown.lock().unwrap().push(request);
        self.showed.notify_one();
        let action = match self.user {
            User::EntersKeys => {
                *self.browser.lock().unwrap() = Some(tokio::spawn(enter_keys(url)));
                ElicitationAction::Accept(ElicitationAcceptAction::new())
            }
            User::Declines => ElicitationAction::Decline,
            User::Dismisses => ElicitationAction::Cancel,
            User::Leaves => return Box::pin(std::future::pending()),
        };
        Box::pin(async move { Ok(CreateElicitationResponse::new(action)) })
    }

    fn complete_elicitation(
        &self,
        notification: CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        self.completed.lock().unwrap().push(notification);
        Box::pin(async { Ok(()) })
    }
}

/// Opens the key page at `url`, submits [`BAD_KEY`], then [`GOOD_KEY`]
/// unless the page already took the first.
async fn enter_keys(url: String) -> Pages {
    let url = Url::parse(&url).unwrap();
    let host = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    let mut pages = Vec::new();
    let opened = request(
        &host,
        &format!("GET {} HTTP/1.1\r\nHost: {host}\r\n\r\n", url.path()),
    )
    .await;
    let mut form = form_action(&opened.1);
    pages.push(opened);
    for key in [BAD_KEY, GOOD_KEY] {
        let body = format!("key={key}");
        let page = request(
            &host,
            &format!(
                "POST {form} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
        .await;
        let done = !page.1.contains("action=");
        pages.push(page);
        if done {
            break;
        }
        form = form_action(&pages.last().unwrap().1);
    }
    pages
}

async fn request(host: &str, request: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(host).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, response)
}

fn form_action(page: &str) -> String {
    let start = page.find(r#"action=""#).expect("a form") + r#"action=""#.len();
    page[start..][..page[start..].find('"').unwrap()].to_owned()
}

// Covers: the host gets only the key page's link, scoped to `authenticate`;
// a key the provider refuses is reported on the page and never saved; the
// key the provider accepts is saved and completes the sign-in; no key ever
// appears in an ACP message, even when the provider quotes it back.
// Owner: ACP API-key authenticate; the provider's models endpoint is local.
#[tokio::test]
async fn a_key_is_checked_on_the_page_and_saved_only_when_accepted() {
    let (endpoint, server) = models_endpoint("401 Unauthorized").await;
    let host = Host::new(User::EntersKeys);
    let store = RecordingStore::default();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        KeyLogin::prepare(AUTH).unwrap().sign_in(
            Some(&endpoint),
            &store,
            RequestId::Number(7),
            &host,
        ),
    )
    .await
    .expect("sign-in ends once the good key is saved");
    server.abort();
    assert_eq!(result, Ok(()));

    let account = match provider::resolve_auth_mode(AUTH).unwrap().1.auth_kind {
        ProviderAuthKind::ApiKey { account, .. } => account,
        kind => panic!("{AUTH} takes an API key: {kind:?}"),
    };
    assert_eq!(
        *store.saved.lock().unwrap(),
        vec![(account.to_owned(), GOOD_KEY.to_owned())],
        "only the accepted key is saved"
    );

    let browser = host.browser.lock().unwrap().take().unwrap();
    let pages = browser.await.unwrap();
    let statuses = pages.iter().map(|(status, _)| *status).collect::<Vec<_>>();
    assert_eq!(statuses, vec![200, 200, 200]);
    assert!(
        pages[1].1.contains("Moonshot AI refused this key."),
        "the refusal is on the page: {}",
        pages[1].1
    );

    let shown = host.shown.into_inner().unwrap();
    let [request] = shown.as_slice() else {
        panic!("one elicitation per sign-in: {shown:?}");
    };
    let ElicitationMode::Url(mode) = &request.mode else {
        unreachable!()
    };
    let completed = host.completed.into_inner().unwrap();
    assert_eq!(
        completed
            .iter()
            .map(|done| done.elicitation_id.clone())
            .collect::<Vec<_>>(),
        vec![mode.elicitation_id.clone()]
    );
    let sent = serde_json::to_string(&(request, &completed)).unwrap();
    assert!(sent.contains(r#""requestId":7"#), "{sent}");
    assert!(
        url::Url::parse(&mode.url)
            .unwrap()
            .host_str()
            .is_some_and(|host| host == "127.0.0.1"),
        "{sent}"
    );
    for key in [GOOD_KEY, BAD_KEY] {
        assert!(!sent.contains(key), "no key in ACP messages: {sent}");
    }
}

// Covers: declining or dismissing the key page's link, or the host leaving
// with `$/cancel_request`, ends the sign-in, saves nothing, and frees the
// page's port.
// Owner: ACP API-key authenticate cancellation.
#[tokio::test]
async fn leaving_the_key_page_frees_its_port() {
    for user in [User::Declines, User::Dismisses, User::Leaves] {
        let host = Host::new(user);
        let store = RecordingStore::default();
        let signing_in =
            KeyLogin::prepare(AUTH)
                .unwrap()
                .sign_in(None, &store, RequestId::Number(7), &host);
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            match user {
                // `$/cancel_request` drops the request's future.
                User::Leaves => tokio::select! {
                    result = signing_in => result,
                    () = host.showed.notified() => Err(AcpError::request_cancelled()),
                },
                User::Declines | User::Dismisses | User::EntersKeys => signing_in.await,
            }
        })
        .await
        .expect("sign-in ends when the user leaves");
        assert_eq!(result, Err(AcpError::request_cancelled()), "{user:?}");
        assert_eq!(*store.saved.lock().unwrap(), Vec::new(), "{user:?}");

        let shown = host.shown.into_inner().unwrap();
        let ElicitationMode::Url(mode) = &shown[0].mode else {
            unreachable!()
        };
        let port = Url::parse(&mode.url).unwrap().port().unwrap();
        TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap_or_else(|error| panic!("{user:?} freed the page's port: {error}"));
    }
}

// Covers: a key the provider cannot be asked about (a gateway without
// `/models`, an outage) is saved unchecked rather than refused forever, so
// such a provider can still be signed in to from the host.
// Owner: ACP API-key authenticate check policy.
#[tokio::test]
async fn a_key_that_cannot_be_checked_is_saved_unchecked() {
    let (endpoint, server) = models_endpoint("404 Not Found").await;
    let host = Host::new(User::EntersKeys);
    let store = RecordingStore::default();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        KeyLogin::prepare(AUTH).unwrap().sign_in(
            Some(&endpoint),
            &store,
            RequestId::Number(7),
            &host,
        ),
    )
    .await
    .expect("sign-in ends once the key is saved");
    server.abort();
    assert_eq!(result, Ok(()));
    let account = match provider::resolve_auth_mode(AUTH).unwrap().1.auth_kind {
        ProviderAuthKind::ApiKey { account, .. } => account,
        kind => panic!("{AUTH} takes an API key: {kind:?}"),
    };
    assert_eq!(
        *store.saved.lock().unwrap(),
        vec![(account.to_owned(), BAD_KEY.to_owned())]
    );
    let browser = host.browser.lock().unwrap().take().unwrap();
    let pages = browser.await.unwrap();
    assert_eq!(pages.len(), 2, "the first key ended the page");
    assert!(!pages[1].1.contains(BAD_KEY), "the key is never echoed");
}
