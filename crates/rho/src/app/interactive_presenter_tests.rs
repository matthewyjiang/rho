use super::*;

// Covers: large apply_patch preview checkpoints grow with buffer size
// Owner: interactive presenter
#[test]
fn apply_patch_preview_stride_grows_with_input_size() {
    let limit = APPLY_PATCH_STREAM_PREVIEW_LIMIT;
    let min_stride = APPLY_PATCH_STREAM_PREVIEW_STRIDE;

    assert_eq!(
        ToolKind::ApplyPatch.preview_parse_stride(limit),
        limit.max(min_stride)
    );
    assert_eq!(
        ToolKind::ApplyPatch.preview_parse_stride(limit * 2),
        (limit * 2).max(min_stride)
    );
    assert_eq!(
        ToolKind::ApplyPatch.preview_parse_stride(limit * 3),
        (limit * 3).max(min_stride)
    );

    // Geometric checkpoints: each interval is at least as large as the last.
    let mut size = limit;
    let mut previous_stride = 0usize;
    for _ in 0..4 {
        let stride = ToolKind::ApplyPatch.preview_parse_stride(size);
        assert!(stride >= min_stride);
        assert!(stride >= previous_stride);
        previous_stride = stride;
        size = size.saturating_add(stride);
    }
}

// Covers: below the apply_patch limit, cadence matches the generic policy
// Owner: interactive presenter
#[test]
fn apply_patch_preview_uses_generic_cadence_below_stream_limit() {
    assert_eq!(ToolKind::ApplyPatch.preview_parse_stride(0), 0);
    assert_eq!(
        ToolKind::ApplyPatch.preview_parse_stride(PREVIEW_FULL_PARSE_LIMIT - 1),
        0
    );
    assert_eq!(
        ToolKind::ApplyPatch.preview_parse_stride(PREVIEW_FULL_PARSE_LIMIT),
        PREVIEW_LARGE_PARSE_STRIDE
    );
    assert_eq!(
        ToolKind::ApplyPatch.preview_parse_stride(APPLY_PATCH_STREAM_PREVIEW_LIMIT - 1),
        PREVIEW_LARGE_PARSE_STRIDE
    );
}
