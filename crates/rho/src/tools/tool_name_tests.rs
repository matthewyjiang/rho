use pretty_assertions::assert_eq;

use super::{cap_tool_name, capped_body, sanitize_segment, HASH_SUFFIX_LEN, MAX_TOOL_NAME_LEN};

/// Returns the readable prefix of a capped name after checking its suffix.
fn capped_prefix(name: &str) -> String {
    let (prefix, hash) = name.rsplit_once('-').expect("capped name has a suffix");
    assert_eq!(
        (
            hash.len(),
            hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        ),
        (HASH_SUFFIX_LEN, true),
        "suffix of {name}"
    );
    prefix.to_owned()
}

// Covers: OpenAPI operation ids and paths become readable segments that never
// contain the `__` separator; lossy inputs may collide and owners must dedupe.
// Owner: shared generated tool-name sanitizer.
#[test]
fn sanitize_segment_keeps_alphanumerics_and_collapses_the_rest() {
    let cases = [
        ("listIssues", "listIssues"),
        ("repos/get-content", "repos_get_content"),
        ("/repos/{owner}/{repo}/pulls", "repos_owner_repo_pulls"),
        ("__snake__case__", "snake_case"),
        ("a.b c", "a_b_c"),
        // Distinct raw ids collide after sanitizing.
        ("a-b", "a_b"),
        ("a/b", "a_b"),
    ];
    for (raw, expected) in cases {
        assert_eq!((raw, sanitize_segment(raw).as_str()), (raw, expected));
    }

    // Text without usable characters still yields a stable, distinct segment.
    let [first, second, repeat] = ["日本", "中文", "日本"].map(sanitize_segment);
    assert_eq!(
        (
            first.len(),
            first.bytes().all(|byte| byte.is_ascii_hexdigit()),
            first == second,
            first == repeat,
        ),
        (HASH_SUFFIX_LEN, true, false, true)
    );
}

// Covers: provider-limit capping leaves names within the limit alone, keeps a
// readable prefix otherwise, and distinguishes long names sharing that prefix.
// Owner: shared generated tool-name sanitizer.
#[test]
fn cap_tool_name_fits_the_provider_limit() {
    let exact = format!("openapi__github__{}", "a".repeat(MAX_TOOL_NAME_LEN - 17));
    assert_eq!(exact.len(), MAX_TOOL_NAME_LEN);
    assert_eq!(cap_tool_name(exact.clone()), exact);

    let shared = format!("openapi__github__{}", "b".repeat(60));
    let [over_by_one, long_a, long_b] = [
        format!("{exact}z"),
        format!("{shared}_create"),
        format!("{shared}_delete"),
    ]
    .map(|name| (name.clone(), cap_tool_name(name)));
    for (original, capped) in [&over_by_one, &long_a, &long_b] {
        let prefix = capped_prefix(capped);
        assert_eq!(
            (
                capped.len(),
                original.starts_with(&prefix),
                capped_body(capped),
                capped_body(original),
            ),
            (MAX_TOOL_NAME_LEN, true, Some(prefix.as_str()), None),
            "{original} -> {capped}"
        );
    }
    assert_ne!(long_a.1, long_b.1);
    assert_eq!(cap_tool_name(long_a.0.clone()), long_a.1);

    // A cut landing on `_` must not leave an empty trailing segment.
    let cut = MAX_TOOL_NAME_LEN - HASH_SUFFIX_LEN - 1;
    let at_separator = format!("{}__{}", "c".repeat(cut - 1), "d".repeat(20));
    assert_eq!(
        capped_prefix(&cap_tool_name(at_separator)),
        "c".repeat(cut - 1)
    );

    // A cut inside a multibyte character backs up to its boundary.
    let multibyte = format!("{}é{}", "e".repeat(cut - 1), "f".repeat(20));
    assert_eq!(
        capped_prefix(&cap_tool_name(multibyte)),
        "e".repeat(cut - 1)
    );
}
