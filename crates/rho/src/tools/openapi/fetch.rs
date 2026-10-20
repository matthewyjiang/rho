//! Spec bytes for a source: a local file, or a download cached under
//! `~/.rho/cache/openapi/` and revalidated with its `ETag` every session.
//!
//! A failed download falls back to the cached copy with a warning, so a
//! flaky network or a moved spec does not take a working source away. A
//! fresh download is cached only once it parses ([`SpecCache::remember`]),
//! so a broken response never replaces the last working copy.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::{
    header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

use super::{
    config::SpecLocation,
    response::{read_body, BodyError},
};

/// Where downloaded specs are kept between sessions.
#[derive(Clone, Debug)]
pub(crate) struct SpecCache {
    dir: PathBuf,
}

/// Validators saved next to a cached spec.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CacheMeta {
    url: String,
    /// SHA-256 of the spec file. Sessions refresh the same entry
    /// concurrently, so a spec read back must match its own validators.
    #[serde(default)]
    sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

impl SpecCache {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `$RHO_HOME/cache/openapi`, or `~/.rho/cache/openapi`.
    pub(crate) fn default_dir() -> Option<PathBuf> {
        crate::paths::rho_dir()
            .ok()
            .map(|rho| rho.join("cache").join("openapi"))
    }

    fn paths(&self, url: &Url) -> (PathBuf, PathBuf) {
        let key = hex::encode(&Sha256::digest(url.as_str().as_bytes())[..16]);
        (
            self.dir.join(format!("{key}.spec")),
            self.dir.join(format!("{key}.json")),
        )
    }

    fn read(&self, url: &Url) -> Option<(CacheMeta, Vec<u8>)> {
        let (spec, meta) = self.paths(url);
        let meta: CacheMeta = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
        (meta.url == url.as_str()).then_some(())?;
        let bytes = std::fs::read(spec).ok()?;
        (meta.sha256 == sha256(&bytes)).then_some((meta, bytes))
    }

    /// Cache a freshly downloaded spec once it has parsed. Failing to cache
    /// costs only the next session's revalidation, so it is logged.
    pub(crate) fn remember(&self, fetched: &FetchedSpec) {
        let Some(meta) = &fetched.fresh else {
            return;
        };
        if let Err(error) = self.write(meta, &fetched.bytes) {
            tracing::warn!(url = %fetched.url, error = %error, "could not cache OpenAPI spec");
        }
    }

    fn write(&self, meta: &CacheMeta, bytes: &[u8]) -> std::io::Result<()> {
        let url = Url::parse(&meta.url).map_err(std::io::Error::other)?;
        let (spec, meta_path) = self.paths(&url);
        let meta = CacheMeta {
            sha256: sha256(bytes),
            ..meta.clone()
        };
        std::fs::create_dir_all(&self.dir)?;
        write_atomically(&self.dir, &spec, bytes)?;
        write_atomically(
            &self.dir,
            &meta_path,
            &serde_json::to_vec(&meta).map_err(std::io::Error::other)?,
        )
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_atomically(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// A spec's bytes and the URL relative server URLs resolve against.
#[derive(Debug)]
pub(crate) struct FetchedSpec {
    pub(crate) bytes: Vec<u8>,
    pub(crate) url: Url,
    /// Set when the download failed and the cached copy was used.
    pub(crate) warning: Option<String>,
    /// Validators of a fresh download, for [`SpecCache::remember`].
    pub(crate) fresh: Option<CacheMeta>,
}

/// What a spec fetch may use.
pub(crate) struct SpecFetch<'a> {
    pub(crate) client: &'a reqwest::Client,
    pub(crate) cache: Option<&'a SpecCache>,
    pub(crate) timeout: Duration,
    pub(crate) max_spec_bytes: usize,
}

impl SpecFetch<'_> {
    pub(crate) async fn fetch(&self, location: &SpecLocation) -> Result<FetchedSpec, String> {
        match location {
            SpecLocation::Path(path) => self.read_file(path),
            SpecLocation::Url(url) => self.download(url).await,
        }
    }

    fn read_file(&self, path: &Path) -> Result<FetchedSpec, String> {
        let read_error =
            |error: std::io::Error| format!("could not read {}: {error}", path.display());
        // Read at most one byte past the limit: metadata alone does not
        // bound a read, since `/dev/zero` reports a size of zero.
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| {
                use std::io::Read as _;
                file.take(self.max_spec_bytes as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(read_error)?;
        if bytes.len() > self.max_spec_bytes {
            let size = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
            return Err(too_large(size.max(bytes.len() as u64), self.max_spec_bytes));
        }
        let absolute = std::path::absolute(path).map_err(read_error)?;
        Ok(FetchedSpec {
            bytes,
            url: Url::from_file_path(&absolute)
                .map_err(|()| format!("{} is not an absolute path", absolute.display()))?,
            warning: None,
            fresh: None,
        })
    }

    async fn download(&self, url: &Url) -> Result<FetchedSpec, String> {
        let cached = self.cache.and_then(|cache| cache.read(url));
        let downloaded = tokio::time::timeout(self.timeout, self.request(url, cached.as_ref()))
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "timed out after {}s (timeout_secs)",
                    self.timeout.as_secs()
                ))
            });
        match (downloaded, cached) {
            (Ok(Download::NotModified), Some((_, bytes))) => Ok(fetched(bytes, url, None, None)),
            (Ok(Download::NotModified), None) => Err(format!(
                "{url} answered 304 Not Modified to a request without a cached copy"
            )),
            (Ok(Download::Fresh { meta, bytes }), _) => Ok(fetched(bytes, url, None, Some(meta))),
            (Err(error), Some((_, bytes))) => Ok(fetched(
                bytes,
                url,
                Some(format!(
                    "could not refresh {url}: {error}; using the cached copy"
                )),
                None,
            )),
            (Err(error), None) => Err(format!("could not download {url}: {error}")),
        }
    }

    async fn request(
        &self,
        url: &Url,
        cached: Option<&(CacheMeta, Vec<u8>)>,
    ) -> Result<Download, String> {
        let mut request = self.client.get(url.clone());
        if let Some((meta, _)) = cached {
            if let Some(etag) = &meta.etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = &meta.last_modified {
                request = request.header(IF_MODIFIED_SINCE, last_modified);
            }
        }
        let response = request
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(Download::NotModified);
        }
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value: &reqwest::header::HeaderValue| value.to_str().ok())
                .map(str::to_owned)
        };
        let meta = CacheMeta {
            url: url.to_string(),
            sha256: String::new(),
            etag: header(ETAG),
            last_modified: header(LAST_MODIFIED),
        };
        let bytes =
            read_body(response, self.max_spec_bytes)
                .await
                .map_err(|error| match error {
                    BodyError::TooLarge { size, limit } => too_large(size, limit),
                    BodyError::Transport(error) => error.without_url().to_string(),
                })?;
        Ok(Download::Fresh { meta, bytes })
    }
}

enum Download {
    NotModified,
    Fresh { meta: CacheMeta, bytes: Vec<u8> },
}

fn fetched(
    bytes: Vec<u8>,
    url: &Url,
    warning: Option<String>,
    fresh: Option<CacheMeta>,
) -> FetchedSpec {
    FetchedSpec {
        bytes,
        url: url.clone(),
        warning,
        fresh,
    }
}

fn too_large(size: u64, limit: usize) -> String {
    format!("OpenAPI spec is {size} bytes, over max_spec_bytes ({limit} bytes)")
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
