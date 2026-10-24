use std::path::Path;

use pretty_assertions::assert_eq;

use super::{file_patch, FileContents};

// Unix paths: on Windows `/work/...` is not absolute and Git-style `\`
// separators need quoting, so no patch is offered there.
#[cfg(unix)]
#[test]
fn patches_follow_git_layout_with_absolute_paths() {
    let cases = [
        (
            "modified file",
            "/work/src/lib.rs",
            FileContents::Modified {
                old: "fn a() {}\nfn b() {}\n",
                new: "fn a() {}\nfn c() {}\n",
            },
            Some(
                "diff --git /work/src/lib.rs /work/src/lib.rs\n\
                 --- /work/src/lib.rs\n\
                 +++ /work/src/lib.rs\n\
                 @@ -1,2 +1,2 @@\n \
                 fn a() {}\n\
                 -fn b() {}\n\
                 +fn c() {}\n",
            ),
        ),
        (
            "created file",
            "/work/notes.md",
            FileContents::Created("hello\n"),
            Some(
                "diff --git /work/notes.md /work/notes.md\n\
                 new file mode 100644\n\
                 --- /dev/null\n\
                 +++ /work/notes.md\n\
                 @@ -0,0 +1 @@\n\
                 +hello\n",
            ),
        ),
        (
            "created empty file",
            "/work/empty",
            FileContents::Created(""),
            Some("diff --git /work/empty /work/empty\nnew file mode 100644\n"),
        ),
        (
            "deleted file",
            "/work/old.txt",
            FileContents::Deleted("bye\n"),
            Some(
                "diff --git /work/old.txt /work/old.txt\n\
                 deleted file mode 100644\n\
                 --- /work/old.txt\n\
                 +++ /dev/null\n\
                 @@ -1 +0,0 @@\n\
                 -bye\n",
            ),
        ),
        (
            "unchanged file",
            "/work/same",
            FileContents::Modified {
                old: "x\n",
                new: "x\n",
            },
            None,
        ),
        (
            "relative path",
            "src/lib.rs",
            FileContents::Modified {
                old: "a\n",
                new: "b\n",
            },
            None,
        ),
        (
            "path Git would quote",
            "/work/\"q\".txt",
            FileContents::Modified {
                old: "a\n",
                new: "b\n",
            },
            None,
        ),
    ];
    for (label, path, contents, expected) in cases {
        assert_eq!(
            file_patch(Path::new(path), contents).as_deref(),
            expected,
            "{label}"
        );
    }
}
