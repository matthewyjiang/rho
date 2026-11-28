//! The stdio transport `rho acp` serves its host on.
//!
//! It frames newline-delimited JSON-RPC as the ACP SDK's `Stdio` does, and
//! adds two things that transport doesn't:
//!
//! - Until an `initialize` succeeds, an [`InitializeScreen`] answers the
//!   `initialize` requests the SDK's protocol router would misroute.
//! - A host that hangs up before reading every reply ends the connection
//!   as cleanly as one that closes stdin. Writing to its closed stdout is
//!   how `rho acp` learns it left.

use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use agent_client_protocol::{
    schema::ProtocolVersion, Channel, ConnectTo, ConnectionDriver, Error as AcpError, Lines, Role,
};
use blocking::Unblock;
use futures_util::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    lock::Mutex as AsyncMutex,
    Sink, Stream, StreamExt,
};

use super::initialize_screen::InitializeScreen;

/// `rho acp`'s end of the host's stdin and stdout.
pub(super) struct HostStdio {
    shared: Arc<Shared>,
}

impl<R: Role> ConnectTo<R> for HostStdio {
    async fn connect_to(self, client: impl ConnectTo<R::Counterpart>) -> Result<(), AcpError> {
        let shared = Arc::clone(&self.shared);
        shared.excuse(ConnectTo::<R>::connect_to(self.lines(), client).await)
    }

    fn into_channel_and_future(self) -> (Channel, Option<ConnectionDriver>) {
        let shared = Arc::clone(&self.shared);
        let (channel, driver) = ConnectTo::<R>::into_channel_and_future(self.lines());
        let driver =
            driver.map(|driver| driver.map_future(|work| async move { shared.excuse(work.await) }));
        (channel, driver)
    }
}

impl HostStdio {
    /// The transport for a router serving the `supported` versions.
    pub(super) fn new(supported: &'static [ProtocolVersion]) -> Self {
        Self {
            shared: Arc::new(Shared {
                stdout: AsyncMutex::new(Unblock::new(std::io::stdout())),
                screen: InitializeScreen::new(supported),
                hung_up: AtomicBool::new(false),
            }),
        }
    }

    fn lines(
        self,
    ) -> Lines<
        impl Sink<String, Error = io::Error> + Send + 'static,
        impl Stream<Item = io::Result<String>> + Send + 'static,
    > {
        let incoming = lossy_lines(Unblock::new(std::io::stdin())).filter_map({
            let shared = Arc::clone(&self.shared);
            move |line| {
                let shared = Arc::clone(&shared);
                async move {
                    let line = match line {
                        Ok(line) => line,
                        Err(error) => return Some(Err(error)),
                    };
                    match shared.screen.incoming(&line) {
                        None => Some(Ok(line)),
                        Some(reply) => shared.write(reply).await.err().map(Err),
                    }
                }
            }
        });
        let outgoing = futures_util::sink::unfold(self.shared, |shared, line: String| async move {
            shared.screen.outgoing(&line);
            shared.write(line).await?;
            Ok::<_, io::Error>(shared)
        });
        Lines::new(Box::pin(outgoing), Box::pin(incoming))
    }
}

/// What the incoming and outgoing halves share.
struct Shared {
    /// Both halves write here: the router's output and the screen's replies.
    stdout: AsyncMutex<Unblock<std::io::Stdout>>,
    screen: InitializeScreen,
    /// Set by a write the host's closed stdout refused.
    hung_up: AtomicBool,
}

impl Shared {
    async fn write(&self, mut line: String) -> io::Result<()> {
        line.push('\n');
        let mut stdout = self.stdout.lock().await;
        let mut written = stdout.write_all(line.as_bytes()).await;
        if written.is_ok() {
            written = stdout.flush().await;
        }
        if written
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
        {
            self.hung_up.store(true, Ordering::Release);
        }
        written
    }

    /// `result`, unless the host hung up. Then nobody is left to tell, so
    /// any error, the broken pipe or what the connection failed with after
    /// it, ends the connection as if the host had closed stdin.
    fn excuse(&self, result: Result<(), AcpError>) -> Result<(), AcpError> {
        match result {
            Err(error) if self.hung_up.load(Ordering::Acquire) => {
                tracing::debug!(%error, "the ACP host hung up; ending the connection");
                Ok(())
            }
            other => other,
        }
    }
}

/// Splits `reader` into lines without their `\n` or `\r\n` endings,
/// replacing bytes that aren't UTF-8 as the SDK's transport does, so one bad
/// byte gets that message an error reply instead of ending the connection.
fn lossy_lines(
    reader: impl AsyncRead + Send + 'static,
) -> impl Stream<Item = io::Result<String>> + Send + 'static {
    let reader = Box::pin(BufReader::new(reader));
    futures_util::stream::try_unfold(reader, |mut reader| async move {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Ok(None);
        }
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        Ok(Some((String::from_utf8_lossy(&line).into_owned(), reader)))
    })
}
