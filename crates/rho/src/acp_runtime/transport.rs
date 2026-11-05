//! Adapt supervised Tokio child pipes to ACP's line transport.

use crate::cli_runtime::{drain::format_line_error, line_decoder::MAX_NDJSON_LINE_BYTES};
use agent_client_protocol::Lines;
use futures_util::{sink, stream, Sink, Stream};
use rho_providers::provider_backend::line_decoder::LineDecoder;
use std::io;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
};

pub(super) fn child_transport(
    stdin: ChildStdin,
    stdout: ChildStdout,
    program: &'static str,
) -> Lines<
    impl Sink<String, Error = io::Error> + Send + 'static,
    impl Stream<Item = io::Result<String>> + Send + 'static,
> {
    let outgoing = sink::unfold(stdin, async |mut writer, line: String| {
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok::<_, io::Error>(writer)
    });
    let incoming = stream::unfold(
        (
            BufReader::new(stdout),
            LineDecoder::with_max_line_bytes(MAX_NDJSON_LINE_BYTES),
            false,
        ),
        async move |(mut reader, mut decoder, mut done)| {
            if done {
                return None;
            }
            loop {
                match decoder.next_line() {
                    Ok(Some(line)) => return Some((Ok(line.to_owned()), (reader, decoder, done))),
                    Err(error) => {
                        return Some((
                            Err(io::Error::other(format_line_error(&error, program))),
                            (reader, decoder, true),
                        ))
                    }
                    Ok(None) => {}
                }
                match reader.fill_buf().await {
                    Ok([]) => {
                        done = true;
                        return match decoder.finish() {
                            Ok(Some(line)) => Some((Ok(line.to_owned()), (reader, decoder, done))),
                            Ok(None) => None,
                            Err(error) => Some((
                                Err(io::Error::other(format_line_error(&error, program))),
                                (reader, decoder, done),
                            )),
                        };
                    }
                    Ok(bytes) => {
                        let length = bytes.len();
                        decoder.push(bytes);
                        reader.consume(length);
                    }
                    Err(error) => return Some((Err(error), (reader, decoder, true))),
                }
            }
        },
    );
    Lines::new(outgoing, incoming)
}
