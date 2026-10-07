//! Bounded Starlark evaluation. Tool I/O stays on the owning Tokio runtime.

use std::sync::{Arc, Mutex};

use super::tool_result::alloc_tool_result;
use super::tools_namespace::tools_namespace;
use super::{bridge::ToolHostBridge, script_output};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};
use starlark::environment::{GlobalsBuilder, LibraryExtension, Module};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::none::NoneType;
use starlark::values::Value;
use starlark::{any::ProvidesStaticType, PrintHandler};

const CODEMODE_DIALECT: Dialect = Dialect {
    enable_top_level_stmt: true,
    enable_f_strings: true,
    ..Dialect::Standard
};

#[derive(Debug, Clone)]
pub(super) struct EngineLimits {
    pub max_ticks: u64,
    pub max_heap_bytes: usize,
    pub max_callstack: usize,
}

impl Default for EngineLimits {
    fn default() -> Self {
        Self {
            max_ticks: 100_000,
            max_heap_bytes: 8 * 1024 * 1024,
            max_callstack: 64,
        }
    }
}

/// Structured result of one script. The model receives only the text from
/// [`format_engine_output`]; `calls` feeds the interactive card's header.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(super) struct EngineOutput {
    pub return_value: JsonValue,
    /// UTF-8 print prefix and any notice, bounded by the canonical output budget.
    pub prints: Vec<String>,
    /// Nested tool calls started, including ones a failure cut short.
    pub calls: usize,
    /// Full multiline Starlark diagnostic; bounded prints and calls remain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One evaluation. Prints survive a failed script so partial work stays visible.
pub(super) struct Evaluation {
    pub result: starlark::Result<JsonValue>,
    pub prints: Vec<String>,
}

#[derive(ProvidesStaticType)]
struct GuestState {
    bridge: Arc<ToolHostBridge>,
    runtime: tokio::runtime::Handle,
}

/// Print strings live outside the Starlark heap. Charge separators too, so even
/// repeated empty prints have bounded storage, and discard all but a prefix.
#[derive(Default)]
struct CapturedPrints {
    lines: Vec<String>,
    received_bytes: usize,
    retained_bytes: usize,
    truncated: bool,
}

impl CapturedPrints {
    fn push(&mut self, text: &str) {
        let separator = usize::from(self.received_bytes != 0 || !self.lines.is_empty());
        self.received_bytes = self
            .received_bytes
            .saturating_add(separator)
            .saturating_add(text.len());
        if self.truncated {
            return;
        }
        let remaining = rho_tools::DEFAULT_MAX_OUTPUT_BYTES - self.retained_bytes;
        if separator <= remaining {
            let prefix = utf8_prefix(text, remaining - separator);
            self.lines.push(prefix.to_owned());
            self.retained_bytes += separator + prefix.len();
        }
        self.truncated = self.received_bytes > rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
    }

    fn into_lines(mut self) -> Vec<String> {
        if !self.truncated {
            return self.lines;
        }
        let limit = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
        let notice = format!(
            "[codemode prints truncated: output byte limit {limit}, received {} bytes]",
            self.received_bytes
        );
        let prefix_budget = limit - notice.len() - 1;
        while self.retained_bytes > prefix_budget {
            let last = self.lines.last_mut().expect("retained prints");
            let excess = self.retained_bytes - prefix_budget;
            if last.len() >= excess {
                let keep = utf8_prefix(last, last.len() - excess).len();
                self.retained_bytes -= last.len() - keep;
                last.truncate(keep);
            } else {
                self.retained_bytes -= last.len();
                self.lines.pop();
                self.retained_bytes -= usize::from(!self.lines.is_empty());
            }
        }
        self.lines.push(notice);
        self.lines
    }
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

struct StatePrint(Mutex<CapturedPrints>);

impl PrintHandler for StatePrint {
    fn println(&self, text: &str) -> starlark::Result<()> {
        self.0.lock().expect("prints").push(text);
        Ok(())
    }
}

/// Called on a blocking-pool thread while the parent task drains host events.
pub(super) fn evaluate_code_mode(
    source: &str,
    bridge: Arc<ToolHostBridge>,
    limits: EngineLimits,
) -> Evaluation {
    let cancellation = bridge.cancellation().clone();
    let state = GuestState {
        bridge,
        runtime: tokio::runtime::Handle::current(),
    };
    let print_handler = StatePrint(Mutex::default());
    let mut builder = GlobalsBuilder::extended_by(&[LibraryExtension::Print]);
    code_mode_api(&mut builder);
    tools_namespace(&mut builder, state.bridge.tool_names());
    let globals = builder.build();
    let result =
        AstModule::parse("codemode.star", source.to_owned(), &CODEMODE_DIALECT).and_then(|ast| {
            Module::with_temp_heap(|module| {
                let mut eval = Evaluator::new(&module);
                eval.extra = Some(&state);
                eval.set_max_tick_count(limits.max_ticks)
                    .map_err(starlark::Error::new_other)?;
                eval.set_max_heap_size(limits.max_heap_bytes)
                    .map_err(starlark::Error::new_other)?;
                eval.set_max_callstack_size(limits.max_callstack)
                    .map_err(starlark::Error::new_other)?;
                eval.set_print_handler(&print_handler);
                eval.set_check_cancelled(Box::new(|| cancellation.is_cancelled()));
                eval.eval_module(ast, &globals)?;
                module
                    .get("result")
                    .map(starlark_to_json)
                    .transpose()
                    .map(|value| value.unwrap_or(JsonValue::Null))
            })
        });
    Evaluation {
        result,
        prints: print_handler.0.into_inner().expect("prints").into_lines(),
    }
}

fn guest<'a>(eval: &'a Evaluator<'_, '_, '_>) -> anyhow::Result<&'a GuestState> {
    eval.extra
        .and_then(|extra| extra.downcast_ref::<GuestState>())
        .ok_or_else(|| anyhow::anyhow!("missing codemode guest state"))
}

/// `{name, description}` rows: discovery stays small; `describe_tool` has schemas.
fn summaries<'v>(
    query: &str,
    limit: i32,
    eval: &mut Evaluator<'v, '_, '_>,
) -> anyhow::Result<Value<'v>> {
    let hits: Vec<_> = guest(eval)?
        .bridge
        .search(query, limit.max(1) as usize)
        .into_iter()
        .map(|entry| json!({"name": entry.name, "description": entry.description}))
        .collect();
    Ok(eval.heap().alloc(JsonValue::Array(hits)))
}

/// One nested call, shared by `call_tool` and `tools.<name>(...)`.
pub(super) fn call_one<'v>(
    name: &str,
    arguments: JsonValue,
    eval: &mut Evaluator<'v, '_, '_>,
) -> anyhow::Result<Value<'v>> {
    let state = guest(eval)?;
    let output = state
        .runtime
        .block_on(state.bridge.call_tool(name, arguments))?;
    Ok(alloc_tool_result(
        eval.heap(),
        &script_output::value(&output),
    ))
}

/// Accepts `name`, `(name,)`, or `(name, args)` per batch item.
fn batch_item(item: JsonValue) -> anyhow::Result<(String, JsonValue)> {
    let invalid = || anyhow::anyhow!("call_tools items must be a name or a (name, args) pair");
    match item {
        JsonValue::String(name) => Ok((name, json!({}))),
        JsonValue::Array(mut parts) if (1..=2).contains(&parts.len()) => {
            let args = if parts.len() == 2 {
                parts
                    .pop()
                    .filter(|args| !args.is_null())
                    .unwrap_or(json!({}))
            } else {
                json!({})
            };
            match parts.pop() {
                Some(JsonValue::String(name)) => Ok((name, args)),
                _ => Err(invalid()),
            }
        }
        _ => Err(invalid()),
    }
}

#[starlark_module]
fn code_mode_api(builder: &mut GlobalsBuilder) {
    fn call_tool<'v>(
        name: &str,
        #[starlark(default = NoneType)] args: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let arguments = if args.is_none() {
            json!({})
        } else {
            starlark_to_json(args).map_err(starlark::Error::into_anyhow)?
        };
        call_one(name, arguments, eval)
    }

    /// Runs independent calls concurrently. A call that cannot complete
    /// becomes an `is_error` envelope so its siblings' results survive.
    fn call_tools<'v>(
        calls: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let items = match starlark_to_json(calls).map_err(starlark::Error::into_anyhow)? {
            JsonValue::Array(items) => items,
            _ => anyhow::bail!("call_tools expects a list of (name, args) pairs"),
        };
        let calls = items
            .into_iter()
            .map(batch_item)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let state = guest(eval)?;
        let results = state.runtime.block_on(state.bridge.call_tools(calls))?;
        let heap = eval.heap();
        let values: Vec<_> = results
            .into_iter()
            .map(|result| {
                let envelope = match result {
                    Ok(output) => script_output::value(&output),
                    Err(error) => script_output::error_value(&error.to_string()),
                };
                alloc_tool_result(heap, &envelope)
            })
            .collect();
        Ok(heap.alloc(values))
    }

    fn search_tools<'v>(
        query: &str,
        #[starlark(default = 10)] limit: i32,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        summaries(query, limit, eval)
    }

    fn list_tools<'v>(
        #[starlark(default = 50)] limit: i32,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        summaries("", limit, eval)
    }

    /// The full catalog entry, with parameter and return schemas, or None.
    fn describe_tool<'v>(
        name: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(match guest(eval)?.bridge.describe(name) {
            Some(entry) => eval.heap().alloc(serde_json::to_value(entry)?),
            None => Value::new_none(),
        })
    }
}

pub(super) fn starlark_to_json(value: Value<'_>) -> starlark::Result<JsonValue> {
    value.to_json_value().map_err(starlark::Error::new_value)
}

pub(super) fn format_engine_output(output: &EngineOutput) -> String {
    let mut parts = Vec::new();
    if !output.prints.is_empty() {
        parts.push(output.prints.join("\n"));
    }
    if !output.return_value.is_null() {
        parts.push(serde_json::to_string_pretty(&output.return_value).expect("JSON value"));
    }
    if let Some(error) = &output.error {
        parts.push(format!("script failed: {error}"));
    }
    let text = if parts.is_empty() {
        format!("(no output; {} nested tool call(s))", output.calls)
    } else {
        parts.join("\n\n")
    };
    let limit = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
    if text.len() <= limit {
        return text;
    }
    let notice = format!(
        "[codemode output truncated: output byte limit {limit}, received {} bytes]",
        text.len()
    );
    if output.error.is_some() {
        // The diagnostic stays last, separated from the print prefix by a blank
        // line. Reserve its bytes before trimming output, not afterwards.
        let failure = parts.pop().expect("script failure");
        let failure_budget = limit - notice.len() - 2;
        if failure.len() > failure_budget {
            // Even the diagnostic alone cannot fit alongside the notice. Keep
            // its head here; the structured `error` still has the full detail.
            return rho_tools::tool::truncate(
                format!("{notice}\n\n{failure}"),
                limit - rho_tools::tool::TRUNCATION_MARKER.len(),
            );
        }
        let body = parts.join("\n\n");
        let prefix_budget = failure_budget.saturating_sub(failure.len() + 1);
        let prefix = utf8_prefix(&body, prefix_budget);
        return if prefix.is_empty() {
            format!("{notice}\n\n{failure}")
        } else {
            format!("{prefix}\n{notice}\n\n{failure}")
        };
    }
    rho_tools::tool::truncate(
        format!("{notice}\n{text}"),
        limit - rho_tools::tool::TRUNCATION_MARKER.len(),
    )
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
