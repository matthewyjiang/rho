//! A one-time local web page that takes an API key.
//!
//! Hosts that can open a URL for the user but log the messages they carry,
//! such as ACP editors, use this so a key reaches Rho without passing through
//! them: the host opens [`KeyEntryPage::url`], and the browser posts the key
//! straight back to this process.
//!
//! The page listens on `127.0.0.1` only. Its URL carries an unguessable token
//! that opens the page once, so a copy of the URL read from a host's logs
//! later finds it gone. The form posts to a second token that never leaves
//! the page, so a rejected key can be corrected and sent again. A request
//! whose `Host` header is not the page's own `127.0.0.1:<port>` is refused,
//! which blocks DNS rebinding. The key never appears in a response, an error, or a log.
//! The page stops answering once a key is accepted, when it expires, or when
//! the [`KeyEntryPage::serve`] future is dropped, which frees the port.

use std::{future::Future, io, time::Duration};

use tokio::{io::AsyncWriteExt, net::TcpListener, net::TcpStream};

use super::loopback::{accept_request, bind_ipv4, callback_url, random_token};

/// How long a client may take to send one request before it is dropped, as
/// for the OpenRouter OAuth callback on the same kind of listener.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const TOKEN_LEN: usize = 32;

/// What the page calls the key and its provider.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyEntryLabels {
    /// The provider, as in "Sign in to OpenAI".
    pub provider: String,
    /// The key's field label, such as "OpenAI API key".
    pub key: String,
}

impl KeyEntryLabels {
    pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            key: key.into(),
        }
    }
}

/// A key the user submitted. It has no `Debug` or `Display`, so it cannot be
/// formatted into a log or error by accident.
pub struct EnteredKey(String);

impl EnteredKey {
    /// The key, trimmed and never empty.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The caller's answer to a submitted key. Reasons and notices are shown on
/// the page and must not contain the key.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyVerdict {
    /// The key is saved; the page says so, with `notice` if any, and stops
    /// answering.
    Accepted { notice: Option<String> },
    /// The page shows this reason and asks again.
    Rejected(String),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KeyEntryError {
    #[error("the key entry page expired after {} minutes", .after.as_secs() / 60)]
    Expired { after: Duration },
    #[error("could not accept a key entry request: {0}")]
    Accept(io::Error),
}

/// A bound, not yet served key entry page.
pub struct KeyEntryPage {
    listener: TcpListener,
    /// `127.0.0.1:<port>`, the only `Host` the page answers.
    authority: String,
    open_token: String,
    url: String,
    labels: KeyEntryLabels,
}

impl KeyEntryPage {
    /// Binds the page to a free loopback port.
    pub async fn bind(labels: KeyEntryLabels) -> io::Result<Self> {
        let listener = bind_ipv4(0).await?;
        let authority = listener.local_addr()?.to_string();
        let open_token = random_token(TOKEN_LEN);
        let url = callback_url(&listener, &format!("/{open_token}"))?;
        Ok(Self {
            listener,
            authority,
            open_token,
            url,
            labels,
        })
    }

    /// The URL that opens the page, once.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Serves the page until `check` accepts a key or `expires_after` passes.
    /// `check` runs one key at a time and should save an accepted key before
    /// it returns, since the page then tells the user they are signed in.
    pub async fn serve<F, Fut>(self, expires_after: Duration, check: F) -> Result<(), KeyEntryError>
    where
        F: FnMut(EnteredKey) -> Fut,
        Fut: Future<Output = KeyVerdict>,
    {
        tokio::time::timeout(expires_after, self.answer_until_accepted(check))
            .await
            .map_err(|_| KeyEntryError::Expired {
                after: expires_after,
            })?
    }

    async fn answer_until_accepted<F, Fut>(&self, mut check: F) -> Result<(), KeyEntryError>
    where
        F: FnMut(EnteredKey) -> Fut,
        Fut: Future<Output = KeyVerdict>,
    {
        let mut state = PageState::Unopened;
        loop {
            let (mut stream, raw) = accept_request(&self.listener, READ_TIMEOUT)
                .await
                .map_err(KeyEntryError::Accept)?;
            let reply = match raw.as_deref().and_then(Request::parse) {
                None => Reply::BadRequest,
                Some(request) => self.answer(&request, &mut state, &mut check).await,
            };
            let accepted = matches!(reply, Reply::Accepted { .. });
            // The user may have closed the tab; the outcome stands either way.
            let _ = self.write(&mut stream, reply).await;
            if accepted {
                return Ok(());
            }
        }
    }

    async fn answer<F, Fut>(
        &self,
        request: &Request<'_>,
        state: &mut PageState,
        check: &mut F,
    ) -> Reply
    where
        F: FnMut(EnteredKey) -> Fut,
        Fut: Future<Output = KeyVerdict>,
    {
        if request.host != Some(self.authority.as_str()) {
            return Reply::WrongHost;
        }
        let token = request.path.strip_prefix('/').unwrap_or_default();
        match (request.method, &*state) {
            ("GET", PageState::Unopened) if token == self.open_token => {
                let form = random_token(TOKEN_LEN);
                let reply = Reply::Form {
                    form: form.clone(),
                    error: None,
                };
                *state = PageState::Open { form };
                reply
            }
            ("GET", PageState::Open { .. }) if token == self.open_token => Reply::Reopened,
            ("POST", PageState::Open { form }) if token == form => {
                let error = match submitted_key(request.body) {
                    None => "Enter a key.".to_owned(),
                    Some(key) => match check(key).await {
                        KeyVerdict::Accepted { notice } => return Reply::Accepted { notice },
                        KeyVerdict::Rejected(reason) => reason,
                    },
                };
                Reply::Form {
                    form: form.clone(),
                    error: Some(error),
                }
            }
            _ => Reply::NotFound,
        }
    }

    async fn write(&self, stream: &mut TcpStream, reply: Reply) -> io::Result<()> {
        let labels = &self.labels;
        let (status, body) = match reply {
            Reply::Form { form, error } => ("200 OK", form_page(labels, &form, error.as_deref())),
            Reply::Accepted { notice } => ("200 OK", done_page(labels, notice.as_deref())),
            Reply::Reopened => (
                "410 Gone",
                message_page(
                    labels,
                    "This sign-in link was already opened. Start signing in again from your editor.",
                ),
            ),
            Reply::WrongHost => (
                "403 Forbidden",
                message_page(labels, "This page only answers at its own address."),
            ),
            Reply::NotFound => ("404 Not Found", message_page(labels, "Not found.")),
            Reply::BadRequest => ("400 Bad Request", message_page(labels, "Bad request.")),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\n\
             content-type: text/html; charset=utf-8\r\n\
             cache-control: no-store\r\n\
             referrer-policy: no-referrer\r\n\
             x-content-type-options: nosniff\r\n\
             content-security-policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'\r\n\
             content-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await
    }
}

/// Where the page is in its one-time life.
enum PageState {
    /// The URL has not been opened yet.
    Unopened,
    /// Opened; the form posts to `form`.
    Open { form: String },
}

enum Reply {
    Form {
        form: String,
        error: Option<String>,
    },
    Accepted {
        notice: Option<String>,
    },
    /// The one-time URL was opened again.
    Reopened,
    WrongHost,
    NotFound,
    BadRequest,
}

/// The parts of an HTTP/1.1 request the page reads.
struct Request<'a> {
    method: &'a str,
    /// The request target without its query.
    path: &'a str,
    host: Option<&'a str>,
    body: &'a str,
}

impl<'a> Request<'a> {
    fn parse(raw: &'a str) -> Option<Self> {
        let (head, body) = raw
            .split_once("\r\n\r\n")
            .or_else(|| raw.split_once("\n\n"))
            .unwrap_or((raw, ""));
        let mut lines = head.lines();
        let mut start = lines.next()?.split_whitespace();
        let method = start.next()?;
        let target = start.next()?;
        let path = target.split('?').next().unwrap_or(target);
        let host = lines.find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("host")
                .then_some(value.trim())
        });
        Some(Self {
            method,
            path,
            host,
            body,
        })
    }
}

/// The trimmed `key` field of a form post, if not blank.
fn submitted_key(body: &str) -> Option<EnteredKey> {
    url::form_urlencoded::parse(body.as_bytes())
        .find(|(name, _)| name == "key")
        .map(|(_, value)| value.trim().to_owned())
        .filter(|key| !key.is_empty())
        .map(EnteredKey)
}

fn form_page(labels: &KeyEntryLabels, form: &str, error: Option<&str>) -> String {
    let key = escape(&labels.key);
    let error = error
        .map(|error| format!(r#"<p class="error" role="alert">{}</p>"#, escape(error)))
        .unwrap_or_default();
    page(
        labels,
        &format!(
            r#"<form method="post" action="/{form}" autocomplete="off">
<label for="key">{key}</label>
<input id="key" name="key" type="password" autocomplete="off" spellcheck="false" autofocus required>
{error}<button type="submit">Save key</button>
</form>
<p class="note">The key goes straight to Rho on this computer and is saved in its credential store.</p>"#
        ),
    )
}

fn done_page(labels: &KeyEntryLabels, notice: Option<&str>) -> String {
    let notice = notice
        .map(|notice| format!(r#"<p class="note">{}</p>"#, escape(notice)))
        .unwrap_or_default();
    page(
        labels,
        &format!("<p>Signed in. You can close this tab and go back to your editor.</p>\n{notice}"),
    )
}

fn message_page(labels: &KeyEntryLabels, message: &str) -> String {
    page(labels, &format!("<p>{}</p>", escape(message)))
}

fn page(labels: &KeyEntryLabels, content: &str) -> String {
    let title = format!("Sign in to {}", escape(&labels.provider));
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
body {{ font-family: system-ui, sans-serif; margin: 0; min-height: 100vh; display: grid; place-items: center; background: #f6f6f4; color: #1d1d1b; }}
main {{ width: min(26rem, calc(100vw - 2rem)); padding: 2rem; background: #fff; border: 1px solid #e2e2dc; border-radius: 12px; }}
h1 {{ font-size: 1.25rem; margin: 0 0 1.25rem; }}
label {{ display: block; font-weight: 600; margin-bottom: 0.5rem; }}
input {{ box-sizing: border-box; width: 100%; padding: 0.6rem 0.7rem; font: inherit; border: 1px solid #c9c9c1; border-radius: 8px; }}
button {{ margin-top: 1rem; padding: 0.6rem 1rem; font: inherit; font-weight: 600; color: #fff; background: #1d1d1b; border: 0; border-radius: 8px; cursor: pointer; }}
.error {{ color: #b3261e; margin: 0.75rem 0 0; }}
.note {{ color: #6b6b63; font-size: 0.875rem; margin: 1.25rem 0 0; }}
@media (prefers-color-scheme: dark) {{
  body {{ background: #161615; color: #ececea; }}
  main {{ background: #1f1f1e; border-color: #33332f; }}
  input {{ background: #161615; color: inherit; border-color: #45453f; }}
  button {{ color: #161615; background: #ececea; }}
  .error {{ color: #f2b8b5; }}
  .note {{ color: #a3a39b; }}
}}
</style>
</head>
<body><main><h1>{title}</h1>
{content}
</main></body>
</html>
"#
    )
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
#[path = "key_entry_tests.rs"]
mod tests;
