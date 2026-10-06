use pretty_assertions::assert_eq;

use super::appended_contents;

// Covers: appending must not merge bullets, erase instructions, or accumulate blank lines.
// Owner: AGENTS.md Markdown assembly.
#[test]
fn appends_one_trimmed_bullet() {
    let cases = [
        (None, "use max jobs 8", "- use max jobs 8\n"),
        (Some(""), "use max jobs 8", "- use max jobs 8\n"),
        (
            Some("# Instructions\n"),
            "be concise",
            "# Instructions\n- be concise\n",
        ),
        (
            Some("- preserve tests"),
            "be concise",
            "- preserve tests\n- be concise\n",
        ),
        (
            Some("- preserve tests\n\n\n"),
            "be concise",
            "- preserve tests\n- be concise\n",
        ),
        (
            Some("- preserve tests\r\n"),
            "  be concise \t",
            "- preserve tests\n- be concise\n",
        ),
        (None, "  be concise \t", "- be concise\n"),
    ];
    for (existing, text, expected) in cases {
        assert_eq!(
            appended_contents(existing, text),
            expected,
            "existing: {existing:?}, text: {text:?}"
        );
    }
}
