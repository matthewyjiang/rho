use pretty_assertions::assert_eq;
use rho_tools::tool_card::{DiffRow, DiffRowKind};

use super::patch_rows;

fn row(kind: DiffRowKind, line: Option<u32>, text: &str) -> DiffRow {
    DiffRow::new(kind, line, text)
}

// Covers: hunk bodies must be counted from their `@@` ranges, so content
// lines that look like headers (`--- x`, `+++ y`) stay content, numbering
// follows each side, and header chrome is hidden or kept as Meta.
// Owner: pure unit (patch parser)
#[test]
fn patch_rows_numbers_hunks_and_keeps_header_lookalikes_as_content() {
    use DiffRowKind::{Added, Context, Meta, Removed, Skip};
    let cases = [
        (
            "two hunks with header lookalikes",
            "diff --git a/f.lua b/f.lua\n\
             index 1111111..2222222 100644\n\
             --- a/f.lua\n\
             +++ b/f.lua\n\
             @@ -1,3 +1,3 @@ local M = {}\n\
             \x20keep\n\
             --- old comment\n\
             +++ new comment\n\
             \x20tail\n\
             @@ -10 +10,2 @@\n\
             -x\n\
             +y\n\
             +z\n",
            vec![
                row(Skip, None, "@@ -1,3 +1,3 @@ local M = {}"),
                row(Context, Some(1), "keep"),
                row(Removed, Some(2), "-- old comment"),
                row(Added, Some(2), "++ new comment"),
                row(Context, Some(3), "tail"),
                row(Skip, None, "@@ -10 +10,2 @@"),
                row(Removed, Some(10), "x"),
                row(Added, Some(10), "y"),
                row(Added, Some(11), "z"),
            ],
        ),
        (
            "new file without trailing newline",
            "diff --git a/n.txt b/n.txt\n\
             new file mode 100644\n\
             index 0000000..3b18e51\n\
             --- /dev/null\n\
             +++ b/n.txt\n\
             @@ -0,0 +1 @@\n\
             +hello\n\
             \\ No newline at end of file\n",
            vec![
                row(Meta, None, "new file mode 100644"),
                row(Skip, None, "@@ -0,0 +1 @@"),
                row(Added, Some(1), "hello"),
                row(Meta, None, "\\ No newline at end of file"),
            ],
        ),
        (
            "no-newline marker between sides and a blank context line",
            "@@ -1,3 +1,3 @@\n\
             \x20a\n\
             \n\
             -b\n\
             \\ No newline at end of file\n\
             +c\n\
             \\ No newline at end of file\n",
            vec![
                row(Skip, None, "@@ -1,3 +1,3 @@"),
                row(Context, Some(1), "a"),
                row(Context, Some(2), ""),
                row(Removed, Some(3), "b"),
                row(Meta, None, "\\ No newline at end of file"),
                row(Added, Some(3), "c"),
                row(Meta, None, "\\ No newline at end of file"),
            ],
        ),
        (
            "binary file",
            "diff --git a/i.png b/i.png\n\
             index 1111111..2222222 100644\n\
             Binary files a/i.png and b/i.png differ\n",
            vec![row(Meta, None, "Binary files a/i.png and b/i.png differ")],
        ),
    ];
    for (name, patch, expected) in cases {
        assert_eq!(patch_rows(patch), expected, "{name}");
    }
}
