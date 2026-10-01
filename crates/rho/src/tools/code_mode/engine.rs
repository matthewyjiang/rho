//! Starlark evaluator for code-mode scripts.
//!
//! Guest globals: `call_tool(name, args=None)` plus captured `print(...)`.
//! Assign `result = ...` for the distilled return value.
//! Nested tool I/O stays on ToolHost; only distillate returns to the model.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use rho_sdk::tool::ToolOutput;
use serde_json::{json, Value as JsonValue};
use starlark::environment::{GlobalsBuilder, LibraryExtension, Module};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::AllocDict;
use starlark::values::list::AllocList;
use starlark::values::none::NoneType;
use starlark::values::{Heap, Value};
use starlark::PrintHandler;
use thiserror::Error;

use super::bridge::{BridgeError, GuardedBridge};
use super::exposure::ExposureController;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("codemode starlark: {0}")]
    Starlark(String),
    #[error(transparent)]
    Bridge(#[from] BridgeError),
    #[error("codemode: {0}")]
    Message(String),
}

#[derive(Debug, Clone)]
pub struct EngineLimits {
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

#[derive(Debug, Clone)]
pub struct EngineOutput {
    pub return_value: JsonValue,
    pub prints: Vec<String>,
    pub nested_calls: usize,
}

struct GuestShared {
    bridge: Arc<GuardedBridge>,
    exposure: Option<Arc<ExposureController>>,
    #[allow(dead_code)]
    prints: Arc<Mutex<Vec<String>>>,
    runtime: tokio::runtime::Handle,
}

thread_local! {
    static GUEST: RefCell<Option<Arc<GuestShared>>> = const { RefCell::new(None) };
}

struct StatePrint {
    #[allow(dead_code)]
    prints: Arc<Mutex<Vec<String>>>,
}

impl PrintHandler for StatePrint {
    fn println(&self, text: &str) -> starlark::Result<()> {
        self.prints.lock().expect("prints").push(text.to_owned());
        Ok(())
    }
}

/// Evaluate a Starlark code-mode body on the current Tokio runtime.
#[cfg(test)]
pub fn evaluate_code_mode(
    source: &str,
    bridge: Arc<GuardedBridge>,
    limits: EngineLimits,
) -> Result<EngineOutput, EngineError> {
    evaluate_code_mode_with_exposure(source, bridge, limits, None)
}

/// Like [`evaluate_code_mode`], with script-side discovery via `search_tools` / `list_tools`.
pub fn evaluate_code_mode_with_exposure(
    source: &str,
    bridge: Arc<GuardedBridge>,
    limits: EngineLimits,
    exposure: Option<Arc<ExposureController>>,
) -> Result<EngineOutput, EngineError> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
        EngineError::Message(
            "codemode requires a Tokio runtime (run inside an async tool call)".into(),
        )
    })?;

    let prints = Arc::new(Mutex::new(Vec::new()));
    let state = Arc::new(GuestShared {
        bridge: bridge.clone(),
        exposure,
        prints: Arc::clone(&prints),
        runtime,
    });
    let print_handler = StatePrint {
        prints: Arc::clone(&prints),
    };

    let globals = {
        let mut builder = GlobalsBuilder::extended_by(&[LibraryExtension::Print]);
        code_mode_api(&mut builder);
        builder.build()
    };

    let ast = AstModule::parse("codemode.star", source.to_owned(), &Dialect::Standard)
        .map_err(|error| EngineError::Starlark(error.to_string()))?;

    GUEST.with(|slot| {
        *slot.borrow_mut() = Some(Arc::clone(&state));
    });

    let return_value = Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        eval.set_max_tick_count(limits.max_ticks)
            .map_err(|error| EngineError::Starlark(error.to_string()))?;
        eval.set_max_heap_size(limits.max_heap_bytes)
            .map_err(|error| EngineError::Starlark(error.to_string()))?;
        eval.set_max_callstack_size(limits.max_callstack)
            .map_err(|error| EngineError::Starlark(error.to_string()))?;
        eval.set_print_handler(&print_handler);
        eval.eval_module(ast, &globals)
            .map_err(|error| EngineError::Starlark(error.to_string()))?;
        let value = match module.get("result") {
            Some(value) => starlark_to_json(value).unwrap_or(JsonValue::Null),
            None => JsonValue::Null,
        };
        Ok::<JsonValue, EngineError>(value)
    });

    GUEST.with(|slot| {
        *slot.borrow_mut() = None;
    });

    let return_value = return_value?;
    let prints = prints.lock().expect("prints").clone();
    Ok(EngineOutput {
        return_value,
        prints,
        nested_calls: bridge.call_count(),
    })
}

#[starlark_module]
fn code_mode_api(builder: &mut GlobalsBuilder) {
    /// Invoke any ToolHost-registered tool (native or MCP) by name.
    fn call_tool<'v>(
        name: &str,
        #[starlark(default = NoneType)] args: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let arguments = if args.is_none() {
            json!({})
        } else {
            starlark_to_json(args).map_err(|error| anyhow::Error::msg(error.to_string()))?
        };
        let output = invoke_blocking(name, arguments)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?;
        let payload = tool_output_to_json(&output);
        let heap = eval.heap();
        Ok(json_to_starlark(heap, &payload))
    }

    /// Keyword/substring search over indexed tools (incl. MCP defaults that are not LLM-declared).
    fn search_tools<'v>(
        query: &str,
        #[starlark(default = 10)] limit: i32,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let hits = discovery_search(query, limit.max(1) as usize)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?;
        let payload = JsonValue::Array(
            hits.into_iter()
                .map(|hit| {
                    json!({
                        "name": hit.name,
                        "description": hit.description,
                    })
                })
                .collect(),
        );
        Ok(json_to_starlark(eval.heap(), &payload))
    }

    /// List tools visible to scripts (excludes hidden). Prefer search_tools for large catalogs.
    fn list_tools<'v>(
        #[starlark(default = 50)] limit: i32,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let hits = discovery_list(limit.max(1) as usize)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?;
        let payload = JsonValue::Array(
            hits.into_iter()
                .map(|hit| {
                    json!({
                        "name": hit.name,
                        "description": hit.description,
                    })
                })
                .collect(),
        );
        Ok(json_to_starlark(eval.heap(), &payload))
    }
}

fn discovery_search(
    query: &str,
    limit: usize,
) -> Result<Vec<super::exposure::ToolCatalogEntry>, EngineError> {
    let state = GUEST
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| EngineError::Message("internal: missing codemode guest state".into()))?;
    let Some(exposure) = state.exposure.as_ref() else {
        return Ok(Vec::new());
    };
    Ok(exposure.search(query, limit))
}

fn discovery_list(limit: usize) -> Result<Vec<super::exposure::ToolCatalogEntry>, EngineError> {
    let state = GUEST
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| EngineError::Message("internal: missing codemode guest state".into()))?;
    let Some(exposure) = state.exposure.as_ref() else {
        return Ok(Vec::new());
    };
    Ok(exposure.list_script_visible(limit))
}

fn invoke_blocking(name: &str, arguments: JsonValue) -> Result<ToolOutput, EngineError> {
    let state = GUEST
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| EngineError::Message("internal: missing codemode guest state".into()))?;
    let bridge = Arc::clone(&state.bridge);
    let name = name.to_owned();
    let output = tokio::task::block_in_place(|| {
        state
            .runtime
            .block_on(async move { bridge.call_tool(&name, arguments).await })
    })?;
    Ok(output)
}

fn tool_output_to_json(output: &ToolOutput) -> JsonValue {
    json!({ "content": output.content() })
}

fn starlark_to_json(value: Value<'_>) -> Result<JsonValue, EngineError> {
    value
        .to_json_value()
        .map_err(|error| EngineError::Starlark(error.to_string()))
}

fn json_to_starlark<'v>(heap: Heap<'v>, value: &JsonValue) -> Value<'v> {
    match value {
        JsonValue::Null => Value::new_none(),
        JsonValue::Bool(flag) => heap.alloc(*flag),
        JsonValue::Number(number) => {
            if let Some(integer) = number.as_i64() {
                heap.alloc(integer)
            } else if let Some(float) = number.as_f64() {
                heap.alloc(float.to_string())
            } else {
                heap.alloc(number.to_string())
            }
        }
        JsonValue::String(text) => heap.alloc(text.as_str()),
        JsonValue::Array(items) => heap.alloc(AllocList(
            items.iter().map(|item| json_to_starlark(heap, item)),
        )),
        JsonValue::Object(map) => heap
            .alloc(AllocDict(map.iter().map(|(key, item)| {
                (heap.alloc(key.as_str()), json_to_starlark(heap, item))
            }))),
    }
}

/// Format engine output for the outer tool result (distilled).
pub fn format_engine_output(output: &EngineOutput) -> String {
    let mut parts = Vec::new();
    if !output.prints.is_empty() {
        parts.push(output.prints.join("\n"));
    }
    if !output.return_value.is_null() {
        parts.push(
            serde_json::to_string_pretty(&output.return_value)
                .unwrap_or_else(|_| output.return_value.to_string()),
        );
    }
    if parts.is_empty() {
        format!("(no output; {} nested tool call(s))", output.nested_calls)
    } else {
        parts.join("\n\n")
    }
}
