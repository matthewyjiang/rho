use pretty_assertions::assert_eq;

use super::ScriptStream;

// Covers: split JSON escapes corrupt literal source, including surrogate pairs,
// or a nested script-like field is mistaken for the real top-level script.
// Owner: incremental script decoder; PTY owns visibility, not escape semantics.
#[test]
fn script_string_decodes_across_every_argument_split() {
    for (arguments, expected) in [
        (
            r#"{"script":"print(\"hello\")\nresult = 1"}"#,
            vec!["print(\"hello\")", "result = 1"],
        ),
        (
            r#"{"script":"a\\b\/c\tend\r\nnext  \n\n"}"#,
            vec!["a\\b/c\tend", "next"],
        ),
        (
            r#"{"script":"print(\"\uD83D\uDE80\")\n"}"#,
            vec!["print(\"🚀\")"],
        ),
        (
            r#"{"nested":{"script":"wrong"},"scr\u0069pt":"α\nβ","extra":true}"#,
            vec!["α", "β"],
        ),
        (r#"{"script":"old","script":"new"}"#, vec!["new"]),
        (
            "{\"script\":\"a\u{007f}\u{0085}\u{009b}z\"}",
            vec!["a\u{007f}\u{0085}\u{009b}z"],
        ),
        (
            r#"{"script":"a\u007f\u0085\u009bz"}"#,
            vec!["a\u{007f}\u{0085}\u{009b}z"],
        ),
    ] {
        for split in arguments
            .char_indices()
            .map(|(index, _)| index)
            .chain([arguments.len()])
        {
            let mut stream = ScriptStream::default();
            stream.update(&arguments[..split]);
            stream.update(arguments);
            assert_eq!(stream.lines(), expected, "split {split}: {arguments}");
            assert!(
                !stream.update(arguments),
                "unchanged input rebuilt the source"
            );
        }
        let mut stream = ScriptStream::default();
        for end in arguments
            .char_indices()
            .map(|(index, ch)| index + ch.len_utf8())
        {
            stream.update(&arguments[..end]);
        }
        assert_eq!(
            stream.lines(),
            expected,
            "single-character deltas: {arguments}"
        );
    }
}

// Covers: unfinished/invalid escapes must not print wire syntax or erase the
// last decoded prefix. Owner: incremental script decoder.
#[test]
fn incomplete_escape_keeps_only_decoded_source() {
    for suffix in [
        "\\",
        "\\u",
        "\\u12",
        "\\uD83D",
        "\\uD83D\\uDE",
        "\\q",
        "\\uD83Dx",
    ] {
        let mut stream = ScriptStream::default();
        stream.update(&format!("{{\"script\":\"prefix{suffix}"));
        assert_eq!(stream.lines(), vec!["prefix"], "suffix {suffix}");
    }
}
