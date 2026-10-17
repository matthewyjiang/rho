//! Local HTTP/1.1 upload model. No reqwest execution or external network access.
use flate2::read::GzDecoder;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub enum Network {
    Loopback,
    BitsPerSecond(u64),
}

impl Network {
    pub fn name(self) -> String {
        match self {
            Self::Loopback => "unpaced_loopback".to_owned(),
            Self::BitsPerSecond(rate) => format!("modeled_{}mbit", rate / 1_000_000),
        }
    }

    pub fn body_seconds(self, bytes: usize) -> f64 {
        match self {
            Self::Loopback => 0.0,
            Self::BitsPerSecond(rate) => bytes as f64 * 8.0 / rate as f64,
        }
    }
}

pub struct Stub {
    pub address: SocketAddr,
    server: JoinHandle<io::Result<()>>,
}

impl Stub {
    pub fn start(expected: Arc<[u8]>, requests: usize) -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let server = thread::spawn(move || {
            for _ in 0..requests {
                let (stream, _) = listener.accept()?;
                stream.set_nodelay(true)?;
                receive(stream, &expected)?;
            }
            Ok(())
        });
        Ok(Self { address, server })
    }

    pub fn finish(self) -> io::Result<()> {
        self.server
            .join()
            .map_err(|_| io::Error::other("local HTTP stub panicked"))?
    }
}

fn receive(stream: TcpStream, expected: &[u8]) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut length = None;
    let mut gzip = false;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line != "POST /v1/messages HTTP/1.1\r\n" {
        return Err(io::Error::other("unexpected local HTTP request line"));
    }
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::other("EOF before HTTP headers ended"));
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<usize>().map_err(io::Error::other)?);
            } else if name.eq_ignore_ascii_case("content-encoding") {
                if value.trim() != "gzip" {
                    return Err(io::Error::other("unsupported local content encoding"));
                }
                gzip = true;
            }
        }
    }
    let mut wire = vec![0; length.ok_or_else(|| io::Error::other("missing body length"))?];
    reader.read_exact(&mut wire)?;
    let decoded = if gzip {
        let mut decoded = Vec::new();
        GzDecoder::new(wire.as_slice()).read_to_end(&mut decoded)?;
        decoded
    } else {
        wire
    };
    if decoded != expected {
        return Err(io::Error::other(
            "local stub received a different JSON payload",
        ));
    }
    // Ack only after full body reception, decompression, and byte validation.
    reader
        .get_mut()
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
}

pub fn upload(
    address: SocketAddr,
    request: &mut reqwest::Request,
    network: Network,
    chunk_bytes: usize,
) -> io::Result<()> {
    // Eligibility was evaluated against the genuine provider URL. Only now
    // reroute to loopback, without modifying its encoded body or body headers.
    *request.url_mut() = format!("http://{address}/v1/messages")
        .parse()
        .map_err(io::Error::other)?;
    let body = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .ok_or_else(|| io::Error::other("benchmark requires an in-memory body"))?;
    let mut stream = TcpStream::connect(address)?;
    stream.set_nodelay(true)?;
    write!(
        stream,
        "POST /v1/messages HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n"
    )?;
    for (name, value) in request.headers() {
        if name == reqwest::header::CONTENT_LENGTH
            && value.to_str().map_err(io::Error::other)? != body.len().to_string()
        {
            return Err(io::Error::other("helper left a stale Content-Length"));
        }
        write!(stream, "{name}: ")?;
        stream.write_all(value.as_bytes())?;
        stream.write_all(b"\r\n")?;
    }
    if !request
        .headers()
        .contains_key(reqwest::header::CONTENT_LENGTH)
    {
        write!(stream, "Content-Length: {}\r\n", body.len())?;
    }
    stream.write_all(b"\r\n")?;
    let start = Instant::now();
    let mut sent = 0;
    for chunk in body.chunks(chunk_bytes) {
        sent += chunk.len();
        // Release each chunk at its cumulative byte deadline. The final chunk
        // waits too, so sub-chunk payloads still pay their modeled upload time.
        let deadline = Duration::from_secs_f64(network.body_seconds(sent));
        if let Some(wait) = deadline.checked_sub(start.elapsed()) {
            thread::sleep(wait);
        }
        stream.write_all(chunk)?;
    }
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200 OK\r\n") || !response.ends_with(b"\r\n\r\nOK") {
        return Err(io::Error::other(
            "local HTTP stub did not acknowledge the body",
        ));
    }
    Ok(())
}
