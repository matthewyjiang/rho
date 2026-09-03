use rho_providers::provider_backend::line_decoder::LineDecoder;

use super::{LineDecodeError, MAX_NDJSON_LINE_BYTES};

#[test]
fn claude_budget_rejects_oversize_and_accepts_near_1mib_tool_result() {
    // Evidence that the cap is about wire NDJSON size, not display size: a
    // complete tool_result frame near 1 MiB (large Read/Bash output) must
    // parse as a line under the 4 MiB budget.
    let payload_chars = 1024 * 1024 - 256;
    let envelope = format!(
        r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_big","content":"{}"}}]}}}}"#,
        "y".repeat(payload_chars)
    );
    assert!(
        envelope.len() > 1024 * 1024 - 512,
        "fixture should exercise multi-megabyte-class tool_result frames"
    );
    assert!(
        envelope.len() <= MAX_NDJSON_LINE_BYTES,
        "legitimate tool_result must fit the decoder budget"
    );

    let mut decoder = LineDecoder::with_max_line_bytes(MAX_NDJSON_LINE_BYTES);
    decoder.push(envelope.as_bytes());
    decoder.push(b"\n");
    let line = decoder
        .next_line()
        .expect("near-1MiB tool_result is legitimate")
        .expect("line present");
    assert_eq!(line, envelope);

    decoder.push(&vec![b'z'; MAX_NDJSON_LINE_BYTES + 1]);
    decoder.push(b"\n");
    let error = decoder
        .next_line()
        .expect_err("oversize after valid large line");
    assert!(matches!(
        error,
        LineDecodeError::LineTooLong {
            limit: MAX_NDJSON_LINE_BYTES,
            ..
        }
    ));
}
