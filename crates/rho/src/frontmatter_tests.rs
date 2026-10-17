use pretty_assertions::assert_eq;

use super::*;

// Covers: fence detection across line endings and the two failure modes.
// Owner: frontmatter splitting.
#[test]
fn splits_documents_at_fences() {
    for (source, expected) in [
        (
            "---\nname: a\n---\nbody\n",
            Ok(Split {
                yaml: "name: a\n",
                body: "body\n",
            }),
        ),
        (
            "---\r\nname: a\r\n---\r\nbody\r\n",
            Ok(Split {
                yaml: "name: a\r\n",
                body: "body\r\n",
            }),
        ),
        ("---\n---", Ok(Split { yaml: "", body: "" })),
        ("# body\n", Err(FrontmatterError::MissingOpening)),
        ("---\nname: a\n", Err(FrontmatterError::MissingClosing)),
    ] {
        assert_eq!(split(source), expected, "{source:?}");
    }
}
