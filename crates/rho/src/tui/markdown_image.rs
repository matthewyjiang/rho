use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use super::markdown::{is_closing_fence, parse_opening_fence};
use super::{feed_image::FeedImage, Entry};
use ratatui_image::picker::Picker;

const MAX_MARKDOWN_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MARKDOWN_IMAGE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_MARKDOWN_IMAGE_REFERENCES: usize = 8;
const MAX_CONCURRENT_IMAGE_READS: usize = 4;
const IMAGE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// An `![alt](path)` image reference found in assistant markdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MarkdownImageSource {
    pub(super) alt: String,
    pub(super) path: String,
}

/// Collects standalone image blocks, skipping fenced code blocks where image
/// syntax is literal text. Images mixed into prose render as alt text instead.
pub(super) fn collect_markdown_image_sources(text: &str) -> Vec<MarkdownImageSource> {
    let mut sources = Vec::new();
    let mut active_fence = None;
    for line in text.lines() {
        if let Some(fence) = active_fence {
            if is_closing_fence(line, fence) {
                active_fence = None;
            }
            continue;
        }
        if let Some(opening) = parse_opening_fence(line) {
            active_fence = Some(opening);
            continue;
        }
        if let Some(image) = standalone_markdown_image(line) {
            sources.push(image);
        }
    }
    sources
}

/// Parses a line that consists only of an image reference, optionally padded
/// with whitespace. Inline images mixed into prose fall back to alt text.
pub(super) fn standalone_markdown_image(line: &str) -> Option<MarkdownImageSource> {
    let trimmed = line.trim();
    if !trimmed.starts_with("![") {
        return None;
    }
    let (image, range) = next_markdown_image(trimmed)?;
    (range == (0..trimmed.len())).then_some(image)
}

/// Parses the next `![alt](path)` span in `line`.
pub(super) fn next_markdown_image(
    line: &str,
) -> Option<(MarkdownImageSource, std::ops::Range<usize>)> {
    let start = line.find("![")?;
    let label_start = start + 2;
    let close_label = line[label_start..].find(']')? + label_start;
    let target_start = close_label + 2;
    if !line[close_label + 1..].starts_with('(') || target_start >= line.len() {
        return None;
    }

    let mut depth = 1usize;
    let mut escaped = false;
    let mut target_end = None;
    for (offset, ch) in line[target_start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    target_end = Some(target_start + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let target_end = target_end?;
    let alt = line[label_start..close_label].trim();
    let path = line[target_start..target_end].trim();
    (!path.is_empty()).then(|| {
        (
            MarkdownImageSource {
                alt: alt.to_string(),
                path: path.replace("\\(", "(").replace("\\)", ")"),
            },
            start..target_end + 1,
        )
    })
}

/// Resolves an image path from markdown against the session working
/// directory. Absolute paths are used as-is and `~` expands to the home
/// directory; anything else is relative to `cwd`.
pub(super) fn resolve_markdown_image_path(path: &str, cwd: &Path) -> Option<PathBuf> {
    if path.starts_with("file://") {
        return url::Url::parse(path).ok()?.to_file_path().ok();
    }
    if path.contains("://") {
        return None;
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return crate::paths::home_dir().map(|home| home.join(rest));
    }
    if path == "~" {
        return crate::paths::home_dir();
    }
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Some(path)
    } else {
        Some(cwd.join(path))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ImageReference {
    entry_index: usize,
    source_index: usize,
    path: PathBuf,
}

type ImageReadTask = (
    PathBuf,
    tokio::task::JoinHandle<Option<super::feed_image::DecodedFeedImage>>,
);

/// Loads, decodes, and caches a bounded set of markdown images. Render state is
/// created on the UI thread only after background decoding completes.
#[derive(Default)]
pub(super) struct MarkdownImageCache {
    loaded: HashMap<PathBuf, Option<super::feed_image::DecodedFeedImage>>,
    pending: HashSet<PathBuf>,
    queued: VecDeque<PathBuf>,
    allowed_references: HashSet<ImageReference>,
    tasks: Vec<ImageReadTask>,
    picker: Option<Picker>,
}

impl MarkdownImageCache {
    /// Registers image references and starts bounded background loads. Returns
    /// true when a newly registered reference can already render from cache.
    pub(super) fn ensure_loads(
        &mut self,
        entry_index: usize,
        sources: &[MarkdownImageSource],
        cwd: &Path,
        picker: Option<&Picker>,
    ) -> bool {
        let Some(picker) = picker else {
            return false;
        };
        self.picker = Some(picker.clone());
        let mut changed = false;
        for (source_index, source) in sources.iter().enumerate() {
            let Some(path) = resolve_markdown_image_path(&source.path, cwd) else {
                continue;
            };
            let reference = ImageReference {
                entry_index,
                source_index,
                path: path.clone(),
            };
            if self.allowed_references.contains(&reference) {
                continue;
            }
            if self.allowed_references.len() >= MAX_MARKDOWN_IMAGE_REFERENCES {
                break;
            }
            self.allowed_references.insert(reference);
            if self.loaded.get(&path).is_some_and(Option::is_some) {
                changed = true;
                continue;
            }
            if self.loaded.contains_key(&path) || !self.pending.insert(path.clone()) {
                continue;
            }
            self.queued.push_back(path);
        }
        self.start_queued_reads();
        changed
    }

    fn start_queued_reads(&mut self) {
        while self.tasks.len() < MAX_CONCURRENT_IMAGE_READS {
            let Some(path) = self.queued.pop_front() else {
                break;
            };
            let task_path = path.clone();
            let handle = tokio::spawn(async move {
                tokio::time::timeout(IMAGE_READ_TIMEOUT, read_and_decode_image(task_path))
                    .await
                    .ok()
                    .flatten()
            });
            self.tasks.push((path, handle));
        }
    }

    /// Drains finished loads into the cache and starts no work on the UI
    /// thread. Returns whether any reference may now render.
    pub(super) fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut remaining = Vec::with_capacity(self.tasks.len());
        for (path, mut handle) in self.tasks.drain(..) {
            use std::{
                future::Future,
                pin::Pin,
                task::{Context, Poll},
            };
            let waker = std::task::Waker::noop();
            let mut context = Context::from_waker(waker);
            match Pin::new(&mut handle).poll(&mut context) {
                Poll::Ready(Ok(image)) => {
                    self.pending.remove(&path);
                    let cached_bytes = self
                        .loaded
                        .values()
                        .filter_map(Option::as_ref)
                        .map(super::feed_image::DecodedFeedImage::estimated_bytes)
                        .sum::<usize>();
                    let image = image.filter(|image| {
                        cached_bytes.saturating_add(image.estimated_bytes())
                            <= MAX_MARKDOWN_IMAGE_CACHE_BYTES
                    });
                    changed |= image.is_some();
                    self.loaded.insert(path, image);
                }
                Poll::Ready(Err(_)) => {
                    self.pending.remove(&path);
                    self.loaded.insert(path, None);
                }
                Poll::Pending => remaining.push((path, handle)),
            }
        }
        self.tasks = remaining;
        self.start_queued_reads();
        changed
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.tasks.is_empty() || !self.queued.is_empty()
    }

    pub(super) fn clear(&mut self) {
        for (_, task) in self.tasks.drain(..) {
            task.abort();
        }
        self.loaded.clear();
        self.pending.clear();
        self.queued.clear();
        self.allowed_references.clear();
        self.picker = None;
    }

    /// Creates terminal render state for loaded, allowed references. The hard
    /// reference limit bounds the decoded copies retained by history entries.
    pub(super) fn ready_images(
        &self,
        entry_index: usize,
        sources: &[MarkdownImageSource],
        cwd: &Path,
    ) -> Vec<(usize, FeedImage)> {
        let Some(picker) = self.picker.as_ref() else {
            return Vec::new();
        };
        sources
            .iter()
            .enumerate()
            .filter_map(|(source_index, source)| {
                let path = resolve_markdown_image_path(&source.path, cwd)?;
                let reference = ImageReference {
                    entry_index,
                    source_index,
                    path: path.clone(),
                };
                self.allowed_references.contains(&reference).then_some(())?;
                let image = self.loaded.get(&path)?.as_ref()?;
                Some((source_index, image.to_feed_image(picker)))
            })
            .collect()
    }
}

async fn read_and_decode_image(path: PathBuf) -> Option<super::feed_image::DecodedFeedImage> {
    let bytes = read_image_bytes(&path).await?;
    tokio::task::spawn_blocking(move || FeedImage::decode(&bytes).ok())
        .await
        .ok()
        .flatten()
}

async fn read_image_bytes(path: &Path) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MARKDOWN_IMAGE_BYTES {
        return None;
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).await.ok()?;
    let metadata = file.metadata().await.ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MARKDOWN_IMAGE_BYTES {
        return None;
    }

    let mut bytes = Vec::new();
    file.take(MAX_MARKDOWN_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    (bytes.len() as u64 <= MAX_MARKDOWN_IMAGE_BYTES).then_some(bytes)
}

impl super::App {
    pub(super) fn mark_markdown_images_dirty_from(&mut self, entry_index: usize) {
        self.markdown_images_dirty_from = Some(
            self.markdown_images_dirty_from
                .map_or(entry_index, |dirty| dirty.min(entry_index)),
        );
    }

    /// Starts loads only for transcript entries changed since the previous
    /// poll, then applies completed background work to the history cache.
    pub(super) fn poll_markdown_images(&mut self) -> bool {
        let picker = self.image_picker.clone();
        let cwd = self.info.runtime.cwd.clone();
        let mut changed = false;
        if let Some(dirty_from) = self.markdown_images_dirty_from.take() {
            for (index, entry) in self.transcript.iter().enumerate().skip(dirty_from) {
                let Entry::Assistant(text) = entry else {
                    continue;
                };
                let sources = collect_markdown_image_sources(text);
                if sources.is_empty() {
                    continue;
                }
                changed |=
                    self.markdown_images
                        .ensure_loads(index, &sources, &cwd, picker.as_ref());
            }
        }
        changed |= self.markdown_images.poll();
        if changed {
            self.history_lines.invalidate_from(0);
        }
        changed
    }
}

#[cfg(test)]
#[path = "markdown_image_tests.rs"]
mod tests;
