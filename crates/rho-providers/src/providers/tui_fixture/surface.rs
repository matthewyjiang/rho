//! `fixture surface` prompts: the `ui` tool shows an A2UI surface.
//!
//! The tool stays out of context until `tool_search` loads it, so each
//! prompt searches first, as a model does.

use rho_sdk::{
    model::{ModelRequest, ModelResponse},
    ProviderError,
};
use serde_json::{json, Value};

use super::{completed, completed_tool_call, tool_result};

const LOAD_CALL_ID: &str = "tui-fixture-surface-load";
const CALL_ID: &str = "tui-fixture-surface";

/// The words the model input opens a surface action with.
const ACTED: &str = "The user acted on surface `";

/// A form whose `ui` call waits for the user's action.
const FORM: &str = r#"$/name = "Ada"
$/notify = false
$/plan = ["free"]
$/seats = 3
root = Card(Column([
  Text("**Team signup**"),
  Text("Fill this in, then submit.", "caption"),
  TextField("Name", $/name, checks=[?required("a name is required")]),
  CheckBox("Email me updates", $/notify),
  ChoicePicker(label="Plan", variant="mutuallyExclusive", options=[{label: "Free", value: "free"}, {label: "Pro", value: "pro"}], value=$/plan),
  Slider(label="Seats", min=1, max=10, value=$/seats),
  Row([
    Button(Text("Cancel"), action=Event("cancel")),
    Button(Text("Submit"), "primary", Event("submit", {name: $/name, notify: $/notify, plan: $/plan, seats: $/seats}))
  ])
]))"#;

/// Every kind of component, on a surface whose call returns at once.
const GALLERY: &str = r#"$/when = "2026-01-31"
$/tags = ["rust"]
$/note = ""
$/secret = "hunter2"
$/files = [{name: "main.rs"}, {name: "lib.rs"}]
root = Column([
  Row([Icon("info"), Text("**Component gallery**")]),
  Divider(),
  Tabs([
    {title: "Inputs", child: Column([
      DateTimeInput(value=$/when, enableDate=true, label="Due"),
      ChoicePicker(label="Tags", variant="multipleSelection", options=[{label: "Rust", value: "rust"}, {label: "Go", value: "go"}, {label: "Zig", value: "zig"}], value=$/tags, displayStyle="chips"),
      TextField("Note", $/note, "Anything to add?", "longText"),
      TextField("Token", $/secret, variant="obscured")
    ])},
    {title: "Media", child: Column([
      Image("https://example.com/diagram.png", "Architecture diagram"),
      Video("https://example.com/demo.mp4"),
      AudioPlayer("https://example.com/notes.mp3", "Voice notes")
    ])}
  ]),
  List(_template($/files, Text($name))),
  Modal(Button(Text("Details"), "borderless", Event("details")), Text("Shown once the modal opens.")),
  Row([
    Button(Text("Docs"), "borderless", openUrl("https://example.com/docs")),
    Button(Text("Apply"), "primary", Event("apply", {when: $/when, tags: $/tags}))
  ])
])"#;

fn block(prompt: &str) -> Option<(&'static str, bool)> {
    match prompt {
        "fixture surface" => Some((FORM, /*wait*/ true)),
        "fixture surface gallery" => Some((GALLERY, /*wait*/ false)),
        _ => None,
    }
}

/// The next call of a `fixture surface` prompt: the search, then the `ui`
/// call. `None` for another prompt, or once the `ui` call returned.
pub(super) fn call(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    let (express, wait) = block(prompt)?;
    if tool_result(request, CALL_ID).is_some() {
        return None;
    }
    if tool_result(request, LOAD_CALL_ID).is_none() {
        return Some(completed_tool_call(
            LOAD_CALL_ID,
            "tool_search",
            json!({"query": "ui show the user a form"}),
        ));
    }
    Some(completed_tool_call(
        CALL_ID,
        "ui",
        json!({"express": express, "surface": "fixture", "wait": wait}),
    ))
}

/// The reply to a `ui` call's result, or to an action the user took after
/// the call returned: what the model read, short enough for one screen row.
pub(super) fn completion(
    request: &ModelRequest<'_>,
    prompt: &str,
) -> Option<Result<ModelResponse, ProviderError>> {
    if let Some(result) = tool_result(request, CALL_ID) {
        return Some(completed(match serde_json::from_str(&result.content) {
            Ok(action) => format!("surface call returned: {}", summary(&action)),
            Err(_) => format!("surface call failed: {}", result.content),
        }));
    }
    let (_, action) = prompt.split_once(ACTED)?.1.split_once("\n\n")?;
    let action = serde_json::from_str(action.lines().next()?).ok()?;
    Some(completed(format!(
        "surface action delivered: {}",
        summary(&action)
    )))
}

/// An action as `name key=value ...`, or a call's other result as it is.
fn summary(action: &Value) -> String {
    let Some(name) = action.get("name").and_then(Value::as_str) else {
        return action.to_string();
    };
    let context = action.get("context").and_then(Value::as_object);
    std::iter::once(name.to_owned())
        .chain(
            context
                .into_iter()
                .flatten()
                .map(|(key, value)| format!("{key}={value}")),
        )
        .collect::<Vec<_>>()
        .join(" ")
}
