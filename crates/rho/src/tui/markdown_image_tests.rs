use std::path::{Path, PathBuf};

use super::{
    collect_markdown_image_sources, resolve_markdown_image_path, standalone_markdown_image,
    MarkdownImageSource,
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

#[tokio::test]
async fn keeps_source_indices_when_an_earlier_image_fails() {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat};
    use ratatui_image::picker::{Picker, ProtocolType};

    let workspace = tempfile::tempdir().unwrap();
    let image_data = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        32,
        16,
        image::Rgba([20, 40, 60, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image_data.write_to(&mut bytes, ImageFormat::Png).unwrap();
    std::fs::write(workspace.path().join("valid.png"), bytes.into_inner()).unwrap();

    let sources = vec![image("missing", "missing.png"), image("valid", "valid.png")];
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    let mut cache = super::MarkdownImageCache::default();
    cache.ensure_loads(0, &sources, workspace.path(), Some(&picker));

    for _ in 0..100 {
        tokio::task::yield_now().await;
        cache.poll();
        if !cache.has_pending() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let ready = cache.ready_images(0, &sources, workspace.path());
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, 1);
}

#[tokio::test]
async fn bounds_renderable_references_and_drains_the_load_queue() {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat};
    use ratatui_image::picker::{Picker, ProtocolType};

    let workspace = tempfile::tempdir().unwrap();
    let image_data = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        32,
        16,
        image::Rgba([20, 40, 60, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image_data.write_to(&mut bytes, ImageFormat::Png).unwrap();
    for index in 0..10 {
        std::fs::write(
            workspace.path().join(format!("image-{index}.png")),
            bytes.get_ref(),
        )
        .unwrap();
    }

    let sources = (0..10)
        .map(|index| image("preview", &format!("image-{index}.png")))
        .collect::<Vec<_>>();
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    let mut cache = super::MarkdownImageCache::default();
    cache.ensure_loads(0, &sources, workspace.path(), Some(&picker));

    for _ in 0..200 {
        tokio::task::yield_now().await;
        cache.poll();
        if !cache.has_pending() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    assert!(!cache.has_pending());
    assert_eq!(cache.ready_images(0, &sources, workspace.path()).len(), 8);
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
