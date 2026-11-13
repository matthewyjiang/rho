//! The cached append position and parsed tree a `Session`'s clones share,
//! and the stamp that says when the file changed under them.

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::SystemTime,
};

use super::tree::SessionTree;

/// Cached append position and parsed tree shared by all clones of one `Session`.
///
/// After a successful append the file is known to end with a complete,
/// newline-terminated record at `valid_len`, so the next append can skip
/// re-reading the file to find a recoverable end. Any mismatch with the
/// file's actual length (external writer, reopened session) or a failed
/// write clears `valid_len` and falls back to full validation.
///
/// `tree` is the last successfully parsed or mutated `SessionTree`, valid for
/// the recorded [`CacheStamp`]. Writers are serialized on this cursor, so the
/// cache is the sole in-process view of the file. The stamp is taken after
/// the append or load completes; a same-size rewrite in that window can be
/// absorbed into it. The session lease, not the stamp, is what excludes
/// concurrent external writers. Length, mtime, or tail mismatches after that
/// still force a reload.
#[derive(Debug, Default)]
pub(super) struct AppendCursor {
    pub(super) valid_len: Option<u64>,
    tree: Option<SessionTree>,
    cached_stamp: Option<CacheStamp>,
}

impl AppendCursor {
    /// Returns the cached tree when the file still matches the state it was
    /// parsed from; clears the cache otherwise.
    pub(super) fn take_tree(&mut self, path: &Path) -> Option<SessionTree> {
        let valid = self.cached_stamp == Some(CacheStamp::of(path));
        if !valid {
            self.invalidate_tree();
        }
        self.tree.take()
    }

    pub(super) fn store_tree(&mut self, tree: SessionTree, path: &Path) {
        // Re-stamp against current disk state so the next validation reflects
        // the append that just landed, not an older snapshot of the file.
        self.cached_stamp = Some(CacheStamp::of(path));
        self.tree = Some(tree);
    }

    /// Adopts a freshly parsed tree as the cache seed and marks the cursor's
    /// append position valid when the file ends in a complete record.
    pub(super) fn seed_loaded_tree(&mut self, tree: SessionTree, path: &Path) {
        let len = CacheStamp::of(path).len();
        self.store_tree(tree, path);
        if len == 0 || file_ends_with_newline(path) {
            self.valid_len = Some(len);
        }
    }

    pub(super) fn invalidate_tree(&mut self) {
        self.tree = None;
        self.cached_stamp = None;
    }
}

/// Identity of one on-disk transcript state: length, mtime, and a fingerprint
/// of the final bytes. Length alone misses same-size in-place edits; mtime is
/// unreliable on coarse-clock filesystems for rapid successive writes; the
/// tail fingerprint catches both deterministically. Corruption beyond the
/// tail window is caught on the next cold load, e.g. when the session is
/// reopened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CacheStamp {
    len: u64,
    mtime: Option<SystemTime>,
    tail_fingerprint: Option<u64>,
}

/// Bytes of the transcript tail covered by [`CacheStamp`].
const TAIL_FINGERPRINT_BYTES: u64 = 4096;

impl CacheStamp {
    fn of(path: &Path) -> Self {
        let (len, mtime) = match fs::metadata(path) {
            Ok(metadata) => (metadata.len(), metadata.modified().ok()),
            Err(_) => (0, None),
        };
        Self {
            len,
            mtime,
            tail_fingerprint: tail_fingerprint(path),
        }
    }

    fn len(&self) -> u64 {
        self.len
    }
}

/// FNV-1a over the last [`TAIL_FINGERPRINT_BYTES`] bytes of `path`.
fn tail_fingerprint(path: &Path) -> Option<u64> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail = TAIL_FINGERPRINT_BYTES.min(len);
    file.seek(SeekFrom::End(-(tail as i64))).ok()?;
    let mut buf = vec![0u8; tail as usize];
    file.read_exact(&mut buf).ok()?;
    Some(buf.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    }))
}

fn file_ends_with_newline(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::End(-1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    matches!(file.read_exact(&mut last), Ok(())) && last[0] == b'\n'
}
