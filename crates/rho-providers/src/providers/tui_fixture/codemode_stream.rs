//! Marker-gated script arguments: no codemode execution until the JSON closes.

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed, completed_tool_call, release, tool_result, CODEMODE_CALL_ID};

const NEXT: &str = ".rho-fixture-release-codemode-next";
const COMPLETE: &str = ".rho-fixture-release-codemode-complete";
const TARGET: &str = ".rho-tui-fixture-codemode-executed.txt";

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    let large = match prompt {
        "fixture codemode" => false,
        "fixture codemode large script" => true,
        _ => return None,
    };
    if let Some(result) = tool_result(request, CODEMODE_CALL_ID) {
        return Some(completed(if result.ok {
            "codemode fixture complete"
        } else {
            "codemode fixture failed"
        }));
    }
    Some(stream(request, events, large).await)
}

async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
    large: bool,
) -> Result<ModelResponse, ProviderError> {
    release::consume_release(NEXT)?;
    release::consume_release(COMPLETE)?;
    let mut script = String::new();
    if large {
        // Cross the presenter's existing 4096-byte coarse-parse threshold with
        // actual source, then append a small delta at the next checkpoint.
        while script.len() <= 4096 {
            script.push_str("# source padding for the large argument streaming checkpoint\n");
        }
    }
    script.push_str("print(\"FIRST_SCRIPT_FRAGMENT\")\n");
    let encoded = serde_json::to_string(&script).expect("script JSON string");
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(CODEMODE_CALL_ID.into()),
            name: Some("codemode".into()),
            // Leave both the script string and the argument object open.
            arguments: format!("{{\"script\":{}", &encoded[..encoded.len() - 1]),
        })
        .await?;
    release::wait_for_release_or_cancel(NEXT, &request.cancellation).await?;

    let mut next = String::new();
    if !large {
        // Put the next literal below the collapsed card's existing ten-row
        // output budget. A permanently head-clipped preview cannot show it.
        for row in 1..=12 {
            next.push_str(&format!("# generated source row {row:02}\n"));
        }
    }
    next.push_str("print(\"SECOND_SCRIPT_FRAGMENT\")\n");
    script.push_str(&next);
    let encoded = serde_json::to_string(&next).expect("script JSON string");
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: encoded[1..encoded.len() - 1].into(),
        })
        .await?;
    release::wait_for_release_or_cancel(COMPLETE, &request.cancellation).await?;

    // The batch line exceeds the generic 256-byte syntax budget but stays under
    // Python's 4 KiB budget, so it must keep token colors after wrapping.
    let ending = format!(
        "hits = call_tools([(\"list_dir\", {{\"path\": \".\"}}), (\"glob\", {{\"pattern\": \"**/{{AGENTS,CONTRIBUTING,README,SECURITY,CODE_OF_CONDUCT,SUPPORT,ARCHITECTURE,DEVELOPMENT,TESTING,RELEASING,MIGRATIONS,TROUBLESHOOTING,INSTALLATION,CONFIGURATION}}.md\", \"include_hidden\": False, \"max_results\": 100}}), (\"write\", {{\"path\": \"{TARGET}\", \"content\": \"executed\\n\"}})])\nprint(\"codemode fixture batch\", len(hits))\nresult = [h[\"is_error\"] for h in hits]"
    );
    script.push_str(&ending);
    let encoded = serde_json::to_string(&ending).expect("script JSON string");
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: format!("{}\"}}", &encoded[1..encoded.len() - 1]),
        })
        .await?;
    completed_tool_call(
        CODEMODE_CALL_ID,
        "codemode",
        serde_json::json!({"script": script}),
    )
}
