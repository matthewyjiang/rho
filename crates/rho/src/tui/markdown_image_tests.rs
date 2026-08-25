use std::path::{Path, PathBuf};

use super::{
    collect_markdown_image_sources, markdown_image_suffix_may_have_changed,
    resolve_markdown_image_path, standalone_markdown_image, MarkdownImageSource,
};

fn image(alt: &str, path: &str) -> MarkdownImageSource {
    MarkdownImageSource {
        alt: alt.to_string(),
        path: path.to_string(),
    }
}

#[test]
fn collects_standalone_images_and_skips_inline_images() {
    let text = "before\n\n![diagram](docs/arch.png)\n\nsee ![icon](i.png) inline\n";

    assert_eq!(
        collect_markdown_image_sources(text),
        vec![image("diagram", "docs/arch.png")]
    );
}

#[test]
fn skips_images_inside_code_fences() {
    let text = "```\n![fake](nope.png)\n```\n\n![real](yes.png)\n";

    assert_eq!(
        collect_markdown_image_sources(text),
        vec![image("real", "yes.png")]
    );
}

#[test]
fn skips_links_that_are_not_images() {
    let text = "[docs](https://example.com) and plain text";

    assert!(collect_markdown_image_sources(text).is_empty());
}

#[test]
fn requires_a_target() {
    assert!(collect_markdown_image_sources("![alt]()").is_empty());
}

#[test]
fn standalone_requires_only_whitespace_around_the_image() {
    assert_eq!(
        standalone_markdown_image("  ![diagram](docs/arch.png)  "),
        Some(image("diagram", "docs/arch.png"))
    );
    assert_eq!(standalone_markdown_image("see ![icon](i.png)"), None);
    assert_eq!(standalone_markdown_image("[link](x.png)"), None);
    assert_eq!(
        standalone_markdown_image("![plot](plots/run_(1).png)"),
        Some(image("plot", "plots/run_(1).png"))
    );
}

#[test]
fn resolves_paths_against_cwd_absolute_and_home() {
    let cwd = Path::new("/work/project");

    assert_eq!(
        resolve_markdown_image_path("docs/pic.png", cwd),
        Some(PathBuf::from("/work/project/docs/pic.png"))
    );
    assert_eq!(
        resolve_markdown_image_path("/abs/pic.png", cwd),
        Some(PathBuf::from("/abs/pic.png"))
    );

    let home = crate::paths::home_dir();
    if let Some(home) = home {
        assert_eq!(
            resolve_markdown_image_path("~/pic.png", cwd),
            Some(home.join("pic.png"))
        );
    }
}

// Covers: a streamed `)` can complete `![alt](path)` and must dirty image loads.
// Owner: markdown image suffix dirty policy
#[test]
fn suffix_change_detects_a_completed_image_target() {
    assert!(markdown_image_suffix_may_have_changed(
        "![plot](docs/chart.png)",
        ".png)"
    ));
    assert!(markdown_image_suffix_may_have_changed(
        "see below\n![plot](docs/chart.png)",
        "(docs/chart.png)"
    ));
    assert!(!markdown_image_suffix_may_have_changed(
        "plain prose continues here",
        " continues here"
    ));
    assert!(!markdown_image_suffix_may_have_changed(
        "![plot](docs/chart.png)\nmore prose",
        "\nmore prose"
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_a_fifo_before_opening_it() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let workspace = tempfile::tempdir().unwrap();
    let fifo = workspace.path().join("image.pipe");
    let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    let result = unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) };
    assert_eq!(result, 0);

    let read = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        super::read_image_bytes(&fifo),
    )
    .await;
    assert!(matches!(read, Ok(None)));
}
