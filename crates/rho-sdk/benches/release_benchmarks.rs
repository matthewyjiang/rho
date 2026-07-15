use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::{BTreeMap, VecDeque},
    hint::black_box,
    num::NonZeroUsize,
    process::Command,
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use rho_sdk::{
    model::{ContentBlock, Message, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    CompactionFuture, CompactionOutput, CompactionRequest, Compactor, Error, Rho, SessionOptions,
    UserInput,
};
use serde_json::{json, Value};

const EVENT_COUNT: usize = 10_000;
const HISTORY_COUNT: usize = 1_000;

struct TrackingAllocator;

static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

fn record_allocation(bytes: usize) {
    let current = ALLOCATED_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_ALLOCATED_BYTES.fetch_max(current, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        ALLOCATED_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                ALLOCATED_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[derive(Clone)]
struct ImmediateProvider;

impl ModelProvider for ImmediateProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("benchmark", "retained-fixture", "v1")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "ok".into(),
            )]))
        })
    }
}

#[derive(Clone)]
struct EventProvider {
    sent_at: Arc<Mutex<VecDeque<Instant>>>,
}

impl ModelProvider for EventProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("benchmark", "event-fixture", "v1")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )]))
        })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            for _ in 0..EVENT_COUNT {
                self.sent_at
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push_back(Instant::now());
                tokio::select! {
                    result = events.send(ModelEvent::OutputDelta("x".into())) => result?,
                    () = request.cancellation.cancelled() => {
                        return Err(rho_sdk::ProviderError::interrupted("benchmark cancelled"));
                    }
                }
            }
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )]))
        })
    }
}

#[derive(Clone)]
struct BenchCompactor;

impl Compactor for BenchCompactor {
    fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
        Box::pin(async move {
            if request.cancellation().is_cancelled() {
                return Err(Error::Cancelled);
            }
            Ok(compact_messages(request.messages()))
        })
    }
}

fn compact_messages(messages: &[Message]) -> CompactionOutput {
    let bytes = messages
        .iter()
        .map(|message| format!("{message:?}").len())
        .sum::<usize>();
    CompactionOutput::new(vec![Message::System(format!(
        "summary: {} messages, {bytes} debug bytes",
        messages.len()
    ))])
    .unwrap()
}

struct SampleStats {
    samples_ns: Vec<u64>,
}

impl SampleStats {
    fn new(mut samples_ns: Vec<u64>) -> Self {
        samples_ns.sort_unstable();
        Self { samples_ns }
    }

    fn percentile(&self, percentile: usize) -> u64 {
        let index = ((self.samples_ns.len() - 1) * percentile).div_ceil(100);
        self.samples_ns[index]
    }

    fn median(&self) -> u64 {
        self.percentile(50)
    }

    fn json(&self) -> Value {
        json!({
            "unit": "nanoseconds",
            "samples": self.samples_ns,
            "median": self.median(),
            "p95": self.percentile(95),
            "p99": self.percentile(99),
        })
    }
}

fn measure(samples: usize, mut operation: impl FnMut()) -> SampleStats {
    let durations = (0..samples)
        .map(|_| {
            let started = Instant::now();
            operation();
            started.elapsed().as_nanos() as u64
        })
        .collect();
    SampleStats::new(durations)
}

struct RetainedBaselineRuntime {
    provider: Arc<dyn ModelProvider>,
    event_capacity: NonZeroUsize,
    max_steps: NonZeroUsize,
    provider_identity: ModelIdentity,
    tools: Arc<BTreeMap<String, Value>>,
    prompt_sources: Arc<Vec<String>>,
    settings: Arc<RwLock<BTreeMap<String, String>>>,
    lifecycle: Arc<Mutex<BTreeMap<String, bool>>>,
}

struct RetainedBaselineSession {
    runtime: Arc<RetainedBaselineRuntime>,
    history: Mutex<Vec<Message>>,
    identity: String,
    state: AtomicU8,
}

fn retained_baseline_startup() -> Arc<RetainedBaselineSession> {
    let mut settings = BTreeMap::new();
    settings.insert("reasoning".into(), "default".into());
    settings.insert("system_prompt".into(), "none".into());
    let runtime = Arc::new(RetainedBaselineRuntime {
        provider: Arc::new(ImmediateProvider),
        event_capacity: NonZeroUsize::new(64).unwrap(),
        max_steps: NonZeroUsize::new(32).unwrap(),
        provider_identity: ModelIdentity::new("benchmark", "retained-fixture", "v1"),
        tools: Arc::new(BTreeMap::new()),
        prompt_sources: Arc::new(Vec::new()),
        settings: Arc::new(RwLock::new(settings)),
        lifecycle: Arc::new(Mutex::new(BTreeMap::new())),
    });
    Arc::new(RetainedBaselineSession {
        runtime,
        history: Mutex::new(Vec::new()),
        identity: rho_sdk::SessionId::new().into_string(),
        state: AtomicU8::new(0),
    })
}

fn consume_baseline(session: Arc<RetainedBaselineSession>) {
    black_box(session.runtime.provider.identity());
    black_box(session.runtime.event_capacity);
    black_box(session.runtime.max_steps);
    black_box(&session.runtime.provider_identity);
    black_box(&session.runtime.tools);
    black_box(&session.runtime.prompt_sources);
    black_box(
        session
            .runtime
            .settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len(),
    );
    black_box(&session.runtime.lifecycle);
    black_box(&session.history);
    black_box(&session.identity);
    black_box(&session.state);
}

fn history() -> Vec<Message> {
    (0..HISTORY_COUNT)
        .map(|index| {
            if index % 2 == 0 {
                Message::user_text(format!(
                    "representative user message {index}: {}",
                    "x".repeat(64)
                ))
            } else {
                Message::assistant_text(format!(
                    "representative assistant message {index}: {}",
                    "y".repeat(64)
                ))
            }
        })
        .collect()
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

fn machine_metadata() -> Value {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|contents| {
            contents
                .lines()
                .find_map(|line| line.strip_prefix("model name\t: ").map(str::to_owned))
        })
        .unwrap_or_else(|| "unavailable".into());
    let memory_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("MemTotal:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        });
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu": cpu,
        "memory_kib": memory_kib,
        "rustc": command_output("rustc", &["--version"]),
        "cargo": command_output("cargo", &["--version"]),
    })
}

fn main() {
    let samples = std::env::var("RHO_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20usize)
        .max(5);
    let tokio = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();

    for _ in 0..5 {
        consume_baseline(retained_baseline_startup());
        let runtime = Rho::builder().provider(ImmediateProvider).build().unwrap();
        black_box(
            tokio
                .block_on(runtime.session(SessionOptions::default()))
                .unwrap(),
        );
    }

    let startup_baseline = measure(samples, || consume_baseline(retained_baseline_startup()));
    let startup_candidate = measure(samples, || {
        let runtime = Rho::builder().provider(ImmediateProvider).build().unwrap();
        black_box(
            tokio
                .block_on(runtime.session(SessionOptions::default()))
                .unwrap(),
        );
    });

    let simple_baseline = measure(samples, || {
        let messages = vec![Message::user_text("hello"), Message::assistant_text("ok")];
        black_box(messages);
    });
    let simple_candidate = measure(samples, || {
        let runtime = Rho::builder().provider(ImmediateProvider).build().unwrap();
        let session = tokio
            .block_on(runtime.session(SessionOptions::default()))
            .unwrap();
        black_box(tokio.block_on(session.complete("hello")).unwrap());
    });

    let representative_history = history();
    let snapshot_runtime = Rho::builder().provider(ImmediateProvider).build().unwrap();
    let snapshot_session = tokio
        .block_on(
            snapshot_runtime
                .session(SessionOptions::default().history(representative_history.clone())),
        )
        .unwrap();
    let snapshot = measure(samples, || {
        black_box(snapshot_session.snapshot().to_json().unwrap());
    });
    let serialized_size = snapshot_session.snapshot().to_json().unwrap().len();
    let allocation_baseline = ALLOCATED_BYTES.load(Ordering::Relaxed);
    PEAK_ALLOCATED_BYTES.store(allocation_baseline, Ordering::Relaxed);
    black_box(snapshot_session.snapshot().to_json().unwrap());
    let peak_snapshot_allocation = PEAK_ALLOCATED_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(allocation_baseline);
    let retained_allocation_ratio = peak_snapshot_allocation as f64 / serialized_size as f64;

    let mut baseline_compaction_sessions = (0..samples)
        .map(|_| Mutex::new(representative_history.clone()))
        .collect::<VecDeque<_>>();
    let compaction_baseline = measure(samples, || {
        let history = baseline_compaction_sessions.pop_front().unwrap();
        let messages = history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let replacement = compact_messages(&messages).into_messages();
        *history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = replacement;
        black_box(history);
    });
    let compaction_runtime = Rho::builder()
        .provider(ImmediateProvider)
        .compactor(BenchCompactor)
        .build()
        .unwrap();
    let mut compaction_sessions = (0..samples)
        .map(|_| {
            tokio
                .block_on(
                    compaction_runtime
                        .session(SessionOptions::default().history(representative_history.clone())),
                )
                .unwrap()
        })
        .collect::<VecDeque<_>>();
    let compaction_candidate = measure(samples, || {
        let session = compaction_sessions.pop_front().unwrap();
        black_box(tokio.block_on(session.compact()).unwrap());
    });

    let mut event_throughputs = Vec::with_capacity(samples);
    let mut event_p99_latencies = Vec::with_capacity(samples);
    for _ in 0..samples {
        let sent_at = Arc::new(Mutex::new(VecDeque::with_capacity(EVENT_COUNT)));
        let runtime = Rho::builder()
            .provider(EventProvider {
                sent_at: Arc::clone(&sent_at),
            })
            .event_capacity(NonZeroUsize::new(256).unwrap())
            .build()
            .unwrap();
        let session = tokio
            .block_on(runtime.session(SessionOptions::default()))
            .unwrap();
        let mut run = tokio
            .block_on(session.start(UserInput::text("events")))
            .unwrap();
        let mut latencies = Vec::with_capacity(EVENT_COUNT);
        let started = Instant::now();
        tokio.block_on(async {
            while let Some(event) = run.next_event().await {
                if matches!(event, rho_sdk::RunEvent::AssistantTextDelta { .. }) {
                    let sent = sent_at
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .pop_front()
                        .unwrap();
                    latencies.push(sent.elapsed().as_nanos() as u64);
                }
            }
            run.outcome().await.unwrap();
        });
        let elapsed = started.elapsed();
        event_throughputs.push(EVENT_COUNT as f64 / elapsed.as_secs_f64());
        latencies.sort_unstable();
        event_p99_latencies.push(latencies[(EVENT_COUNT - 1) * 99 / 100]);
    }
    event_throughputs.sort_by(f64::total_cmp);
    event_p99_latencies.sort_unstable();
    let event_throughput_median = event_throughputs[event_throughputs.len() / 2];
    let event_latency_p99 = event_p99_latencies[event_p99_latencies.len() / 2];

    let mut slow_consumer_cancellation = Vec::with_capacity(samples);
    for _ in 0..samples {
        let sent_at = Arc::new(Mutex::new(VecDeque::with_capacity(EVENT_COUNT)));
        let runtime = Rho::builder()
            .provider(EventProvider { sent_at })
            .event_capacity(NonZeroUsize::new(1).unwrap())
            .build()
            .unwrap();
        let session = tokio
            .block_on(runtime.session(SessionOptions::default()))
            .unwrap();
        let mut run = tokio
            .block_on(session.start(UserInput::text("slow")))
            .unwrap();
        tokio.block_on(run.next_event());
        tokio.block_on(run.next_event());
        std::thread::sleep(Duration::from_millis(1));
        let started = Instant::now();
        run.cancel();
        let _ = tokio.block_on(run.outcome());
        slow_consumer_cancellation.push(started.elapsed().as_nanos() as u64);
    }
    let slow_consumer = SampleStats::new(slow_consumer_cancellation);

    let startup_relative = startup_candidate.median() as f64 / startup_baseline.median() as f64;
    let simple_allowed =
        (simple_baseline.median() as f64 * 1.10).max(simple_baseline.median() as f64 + 100_000.0);
    let compaction_relative =
        compaction_candidate.median() as f64 / compaction_baseline.median() as f64;
    let checks = json!({
        "startup_absolute_under_2ms": startup_candidate.median() <= 2_000_000,
        "startup_relative_under_20_percent": startup_relative <= 1.20,
        "simple_completion_within_budget": simple_candidate.median() as f64 <= simple_allowed,
        "event_throughput_at_least_250k_per_second": event_throughput_median >= 250_000.0,
        "event_p99_latency_under_5ms": event_latency_p99 <= 5_000_000,
        "snapshot_median_under_10ms": snapshot.median() <= 10_000_000,
        "snapshot_retained_allocation_under_3x": retained_allocation_ratio < 3.0,
        "compaction_relative_under_15_percent": compaction_relative <= 1.15,
        "slow_consumer_cancellation_under_250ms": slow_consumer.percentile(99) <= 250_000_000,
    });
    let passed = checks
        .as_object()
        .unwrap()
        .values()
        .all(|value| value == &Value::Bool(true));

    let evidence = json!({
        "schema_version": 1,
        "suite": "rho-sdk-release-benchmarks",
        "candidate_commit": command_output("git", &["rev-parse", "HEAD"]),
        "baseline": {
            "id": "pre-sdk-retained-fixture-v1",
            "description": "in-process provider, session ownership, history, and direct compaction fixture retained in this benchmark target"
        },
        "command": "RHO_BENCH_SAMPLES=20 cargo bench -p rho-sdk --bench release_benchmarks",
        "profile": "release/bench",
        "sample_count": samples,
        "machine": machine_metadata(),
        "measurements": {
            "startup": {
                "baseline": startup_baseline.json(),
                "candidate": startup_candidate.json(),
                "candidate_over_baseline": startup_relative,
            },
            "simple_completion": {
                "baseline": simple_baseline.json(),
                "candidate": simple_candidate.json(),
                "allowed_candidate_median_ns": simple_allowed,
            },
            "event_delivery": {
                "events_per_sample": EVENT_COUNT,
                "throughput_samples_per_second": event_throughputs,
                "median_events_per_second": event_throughput_median,
                "per_sample_p99_latency_ns": event_p99_latencies,
                "reported_p99_latency_ns": event_latency_p99,
            },
            "snapshot": {
                "messages": HISTORY_COUNT,
                "timing": snapshot.json(),
                "serialized_bytes": serialized_size,
                "peak_additional_allocated_bytes": peak_snapshot_allocation,
                "peak_retained_allocation_ratio": retained_allocation_ratio,
                "allocation_method": "process-global counting allocator around one single-threaded snapshot clone and serialization",
            },
            "compaction": {
                "messages": HISTORY_COUNT,
                "baseline": compaction_baseline.json(),
                "candidate": compaction_candidate.json(),
                "candidate_over_baseline": compaction_relative,
            },
            "slow_consumer": {
                "event_capacity": 1,
                "cancellation": slow_consumer.json(),
            },
        },
        "budget_source": "docs/sdk/performance.md",
        "budget_checks": checks,
        "passed": passed,
    });

    let output = serde_json::to_string_pretty(&evidence).unwrap();
    if let Some(path) = std::env::var_os("RHO_BENCH_OUTPUT") {
        let path = std::path::PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, format!("{output}\n")).unwrap();
    }
    println!("{output}");
    if !passed {
        std::process::exit(2);
    }
}
