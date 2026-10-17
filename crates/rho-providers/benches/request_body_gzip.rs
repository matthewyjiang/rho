//! Manual benchmark: `cargo bench -p rho-providers --bench request_body_gzip`.
//! Requires an explicit `[[bench]] name = "request_body_gzip", harness = false`.
//! No provider calls: requests are serialized over std TCP to 127.0.0.1 only.
use flate2::{write::GzEncoder, Compression};
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use std::{
    hint::black_box,
    io::{self, Write},
    sync::Arc,
    time::Instant,
};

#[path = "request_body_gzip/corpus.rs"]
mod corpus;
// Compile the real private helper, including its URL/size/keep-identity policy.
#[path = "../src/providers/request_body/encoding.rs"]
mod request_body;
#[path = "request_body_gzip/transport.rs"]
mod transport;

#[derive(Clone, Copy)]
enum Mode {
    Identity,
    FastGzip,
    Production,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::FastGzip => "forced_fast_gzip",
            Self::Production => "production_helper",
        }
    }

    fn encode(self, request: &mut reqwest::Request) -> io::Result<()> {
        match self {
            Self::Identity => Ok(()),
            Self::FastGzip => {
                let compressed = fast_gzip(request.body().unwrap().as_bytes().unwrap())?;
                *request.body_mut() = Some(compressed.into());
                request.headers_mut().remove(CONTENT_LENGTH);
                request
                    .headers_mut()
                    .insert(CONTENT_ENCODING, "gzip".parse().unwrap());
                Ok(())
            }
            Self::Production => request_body::encode_request(request),
        }
    }
}

// Counterfactual below the production threshold, using the same flate2 encoder,
// compression level, and Cargo-resolved backend. Above threshold, byte-for-byte
// parity with the real helper is checked before any timing is reported.
fn fast_gzip(body: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(body)?;
    encoder.finish()
}

fn request(body: &[u8]) -> reqwest::Request {
    let mut request = reqwest::Request::new(
        reqwest::Method::POST,
        "https://api.anthropic.com/v1/messages".parse().unwrap(),
    );
    request
        .headers_mut()
        .insert(CONTENT_TYPE, "application/json".parse().unwrap());
    request
        .headers_mut()
        .insert(CONTENT_LENGTH, body.len().into());
    *request.body_mut() = Some(body.to_vec().into());
    request
}

#[derive(Debug)]
struct Config {
    corpus_revision: String,
    cpu_repeats: usize,
    upload_repeats: usize,
    warmups: usize,
    chunk_bytes: usize,
}

impl Config {
    fn parse() -> io::Result<Self> {
        // Sampling choices, not product budgets. Every choice is printed and
        // overridable; increase repeats to investigate noisy measurements.
        let mut config = Self {
            corpus_revision: "HEAD".into(),
            cpu_repeats: 100,
            upload_repeats: 7,
            warmups: 1,
            chunk_bytes: 16 * 1024,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            // Cargo may append --bench even when the libtest harness is off.
            if arg == "--bench" {
                continue;
            }
            if arg == "--corpus-revision" {
                config.corpus_revision = args
                    .next()
                    .ok_or_else(|| io::Error::other("missing value for --corpus-revision"))?;
                continue;
            }
            let field = match arg.as_str() {
                "--cpu-repeats" => &mut config.cpu_repeats,
                "--upload-repeats" => &mut config.upload_repeats,
                "--warmups" => &mut config.warmups,
                "--chunk-bytes" => &mut config.chunk_bytes,
                _ => {
                    return Err(io::Error::other(format!(
                        "unknown benchmark argument: {arg}"
                    )))
                }
            };
            *field = args
                .next()
                .ok_or_else(|| io::Error::other(format!("missing value for {arg}")))?
                .parse()
                .map_err(io::Error::other)?;
            if *field == 0 && arg != "--warmups" {
                return Err(io::Error::other(format!("{arg} must be positive; asked 0")));
            }
        }
        Ok(config)
    }
}

// Median/min/max in microseconds. Timing is monotonic elapsed time for the
// CPU-bound operation, not hardware cycles or OS process CPU accounting.
fn summary(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(f64::total_cmp);
    let middle = samples.len() / 2;
    let median = if samples.len().is_multiple_of(2) {
        (samples[middle - 1] + samples[middle]) / 2.0
    } else {
        samples[middle]
    };
    (median, samples[0], samples[samples.len() - 1])
}

fn main() -> io::Result<()> {
    let config = Config::parse()?;
    let corpus = corpus::Corpus::load(&config.corpus_revision)?;
    println!("# request_body_gzip: local synthetic benchmark; NOT provider inference");
    println!("# revision={} config={config:?}", corpus.revision);
    println!(
        "# compiled_helper_source_sha256={}",
        corpus::sha256(include_bytes!("../src/providers/request_body/encoding.rs"))
    );
    println!(
        "# os={} arch={} profile=bench; record rustc -Vv and cargo tree -i flate2 separately",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("# source=40 fixed committed Rust files via git show; no working-tree files, sessions, auth, or provider requests");
    println!("# targets <16KiB: one-message snapshots; >=16KiB: 40-message coding history; actual bytes printed");
    println!("# gzip=flate2::write::GzEncoder Compression::fast; forced_fast_gzip bypasses production size policy");
    println!("# CPU: request construction/body copy excluded; helper timing includes policy, compression, header/body replacement");
    println!("# upload: encode + fresh TCP connect + HTTP/1.1 headers + paced body + server decode/byte validation + ack; no TLS/RTT/loss/provider queue/inference");
    println!("# pacing=10,000,000 or 100,000,000 bits/s of BODY only; cumulative chunk deadlines (including final chunk); scheduler jitter is included");
    println!("# ordering=rotate three modes each repeat; CPU/upload each discard warmups; no timing pass/fail assertions");
    println!("sizes,target_bytes,identity_bytes,gzip_bytes,helper_bytes,helper_encoding,sha256");
    println!("cpu,target_bytes,mode,repeats,median_us,min_us,max_us");
    println!("upload,target_bytes,network,mode,repeats,wire_body_bytes,ideal_body_us,median_us,min_us,max_us");
    for power in 10..=20 {
        let target = 1usize << power;
        let body = corpus.body(target);
        let candidate = fast_gzip(&body)?;
        let mut production = request(&body);
        Mode::Production.encode(&mut production)?;
        let encoded = production.body().unwrap().as_bytes().unwrap();
        let gzip = production
            .headers()
            .get(CONTENT_ENCODING)
            .is_some_and(|value| value == "gzip");
        if (gzip && encoded != candidate) || (!gzip && encoded != body) {
            return Err(io::Error::other("production helper differs from benchmark fast-gzip/identity; coordinate compressor assumptions before interpreting results"));
        }
        println!(
            "sizes,{target},{},{},{},{},{}",
            body.len(),
            candidate.len(),
            encoded.len(),
            if gzip { "gzip" } else { "identity" },
            corpus::sha256(&body)
        );
        cpu(&body, target, &config)?;
        upload(body.into(), target, &config)?;
    }
    Ok(())
}

fn cpu(body: &[u8], target: usize, config: &Config) -> io::Result<()> {
    let modes = [Mode::Identity, Mode::FastGzip, Mode::Production];
    let mut samples: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
    for repeat in 0..config.warmups + config.cpu_repeats {
        for offset in 0..modes.len() {
            let index = (repeat + offset) % modes.len();
            let mut request = request(body);
            let start = Instant::now();
            modes[index].encode(black_box(&mut request))?;
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
            black_box(&request);
            if repeat >= config.warmups {
                samples[index].push(elapsed);
            }
        }
    }
    for (mode, samples) in modes.into_iter().zip(&mut samples) {
        let (median, min, max) = summary(samples);
        println!(
            "cpu,{target},{},{},{median:.3},{min:.3},{max:.3}",
            mode.name(),
            config.cpu_repeats
        );
    }
    Ok(())
}

fn upload(body: Arc<[u8]>, target: usize, config: &Config) -> io::Result<()> {
    let modes = [Mode::Identity, Mode::FastGzip, Mode::Production];
    let networks = [
        transport::Network::Loopback,
        transport::Network::BitsPerSecond(10_000_000),
        transport::Network::BitsPerSecond(100_000_000),
    ];
    for network in networks {
        let total = (config.warmups + config.upload_repeats) * modes.len();
        let stub = transport::Stub::start(Arc::clone(&body), total)?;
        let mut samples: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
        let mut wire_sizes = [0; 3];
        for repeat in 0..config.warmups + config.upload_repeats {
            for offset in 0..modes.len() {
                let index = (repeat + offset) % modes.len();
                let mut request = request(&body);
                let start = Instant::now();
                modes[index].encode(&mut request)?;
                transport::upload(stub.address, &mut request, network, config.chunk_bytes)?;
                let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
                wire_sizes[index] = request.body().unwrap().as_bytes().unwrap().len();
                if repeat >= config.warmups {
                    samples[index].push(elapsed);
                }
            }
        }
        stub.finish()?;
        for (index, samples) in samples.iter_mut().enumerate() {
            let (median, min, max) = summary(samples);
            let ideal = network.body_seconds(wire_sizes[index]) * 1_000_000.0;
            println!(
                "upload,{target},{},{},{},{},{ideal:.3},{median:.3},{min:.3},{max:.3}",
                network.name(),
                modes[index].name(),
                config.upload_repeats,
                wire_sizes[index]
            );
        }
    }
    Ok(())
}
