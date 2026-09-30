//! Download of the GeoLite2 City database from the cloud of the project.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::geo::GeoPaths;

/// Where the compressed database comes from.
pub const DOWNLOAD_URL: &str = "https://cloud.nginxui.com/geolite/GeoLite2-City.mmdb.xz";

/// The archive is about 19 MB, anything far larger is not the database.
pub const MAX_DOWNLOAD_BYTES: u64 = 64 << 20;

/// The database is about 60 MB unpacked.
pub const MAX_DATABASE_BYTES: u64 = 256 << 20;

/// Keeps two downloads from writing the same file.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Holds the download slot until it is dropped.
pub struct DownloadSlot;

impl DownloadSlot {
    pub fn acquire() -> Option<DownloadSlot> {
        RUNNING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| DownloadSlot)
    }
}

impl Drop for DownloadSlot {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// A download error with the numbers the pages translate.
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadError {
    Download(String),
    Decompress(String),
    CreateFile(String),
    SaveFile(String),
    OpenFile(String),
    FileSize(String),
}

impl DownloadError {
    pub fn code(&self) -> i32 {
        match self {
            Self::Download(_) => 60000,
            Self::Decompress(_) => 60001,
            Self::FileSize(_) => 60003,
            Self::CreateFile(_) => 60004,
            Self::SaveFile(_) => 60005,
            Self::OpenFile(_) => 60006,
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download(m) => write!(f, "failed to download GeoLite2 database: {m}"),
            Self::Decompress(m) => write!(f, "failed to decompress GeoLite2 database: {m}"),
            Self::CreateFile(m) => write!(f, "failed to create file: {m}"),
            Self::SaveFile(m) => write!(f, "failed to save downloaded file: {m}"),
            Self::OpenFile(m) => write!(f, "failed to open file: {m}"),
            Self::FileSize(m) => write!(f, "failed to get file size: {m}"),
        }
    }
}

/// Builds the client. It follows the proxy variables of the environment, which
/// the host sets for a plugin that holds the network permission.
pub fn client() -> Result<reqwest::Client, DownloadError> {
    // The TLS provider is installed once, a second call finds it in place
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(concat!("nginx-ui-log-analytics/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .build()
        .map_err(|e| DownloadError::Download(e.to_string()))
}

/// Downloads the compressed database to `destination`. `progress` gets the
/// percentage of the download.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    mut progress: impl FnMut(f64),
) -> Result<(), DownloadError> {
    if let Some(dir) = destination.parent() {
        std::fs::create_dir_all(dir).map_err(|e| DownloadError::CreateFile(e.to_string()))?;
    }
    let mut response = client.get(url).send().await.map_err(|e| DownloadError::Download(e.to_string()))?;
    if !response.status().is_success() {
        return Err(DownloadError::Download(format!("status code: {}", response.status().as_u16())));
    }
    let total =
        response.content_length().ok_or_else(|| DownloadError::FileSize("the server did not tell the size".into()))?;
    if total > MAX_DOWNLOAD_BYTES {
        return Err(DownloadError::FileSize(format!("the file is too large: {total} bytes")));
    }

    let mut file = std::fs::File::create(destination).map_err(|e| DownloadError::CreateFile(e.to_string()))?;
    let mut done = 0u64;
    let mut reported = 0.0f64;
    let result: Result<(), DownloadError> = async {
        while let Some(chunk) = response.chunk().await.map_err(|e| DownloadError::SaveFile(e.to_string()))? {
            done += chunk.len() as u64;
            if done > total || done > MAX_DOWNLOAD_BYTES {
                return Err(DownloadError::FileSize("the server sent more than it announced".into()));
            }
            file.write_all(&chunk).map_err(|e| DownloadError::SaveFile(e.to_string()))?;
            let percent = if total > 0 { done as f64 / total as f64 * 100.0 } else { 0.0 };
            if percent - reported >= 1.0 || percent >= 100.0 {
                reported = percent;
                progress(percent);
            }
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

/// Counts what is read from it.
struct Counting<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        Ok(n)
    }
}

/// Decompresses the downloaded file next to the final name and renames it when
/// done, so an indexing round that still maps the previous database keeps
/// reading the old file. `progress` gets the percentage read from the archive.
pub fn decompress(paths: &GeoPaths, mut progress: impl FnMut(f64)) -> Result<(), DownloadError> {
    let xz_path = paths.default_xz();
    // Always the default location, a custom database must never be overwritten
    let db_path = paths.default_db();
    let file = std::fs::File::open(&xz_path).map_err(|e| DownloadError::OpenFile(e.to_string()))?;
    let compressed = file.metadata().map_err(|e| DownloadError::FileSize(e.to_string()))?.len().max(1);

    let tmp = db_path.with_extension("mmdb.tmp");
    let mut out = std::fs::File::create(&tmp).map_err(|e| DownloadError::CreateFile(e.to_string()))?;
    let mut decoder = xz2::read::XzDecoder::new(Counting { inner: file, read: 0 });
    let mut buf = vec![0u8; 64 * 1024];
    let mut reported = 0.0f64;
    let mut written = 0u64;
    loop {
        let n = match decoder.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(DownloadError::Decompress(e.to_string()));
            }
        };
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > MAX_DATABASE_BYTES {
            let _ = std::fs::remove_file(&tmp);
            return Err(DownloadError::FileSize("the unpacked database is too large".into()));
        }
        if let Err(e) = out.write_all(&buf[..n]) {
            let _ = std::fs::remove_file(&tmp);
            return Err(DownloadError::SaveFile(e.to_string()));
        }
        let percent = decoder.get_ref().read as f64 / compressed as f64 * 100.0;
        if percent - reported >= 2.0 {
            reported = percent;
            progress(percent.min(100.0));
        }
    }
    drop(out);
    std::fs::rename(&tmp, &db_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        DownloadError::SaveFile(e.to_string())
    })?;
    progress(100.0);
    // The archive is not needed any more
    let _ = std::fs::remove_file(&xz_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_download_runs() {
        let first = DownloadSlot::acquire();
        assert!(first.is_some());
        assert!(DownloadSlot::acquire().is_none());
        drop(first);
        assert!(DownloadSlot::acquire().is_some());
    }

    #[test]
    fn decompress_replaces_the_default_database_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GeoPaths::new(dir.path().to_path_buf(), "");
        // The embedded country database is a valid xz file
        std::fs::write(paths.default_xz(), include_bytes!("../assets/GeoLite2-Country.mmdb.xz")).unwrap();
        std::fs::write(paths.default_db(), b"old").unwrap();
        let mut last = 0.0;
        decompress(&paths, |p| last = p).unwrap();
        assert_eq!(last, 100.0);
        assert!(std::fs::metadata(paths.default_db()).unwrap().len() > 1_000_000);
        assert!(!paths.default_xz().exists());
        assert!(!dir.path().join("GeoLite2-City.mmdb.tmp").exists());
    }

    #[test]
    fn a_broken_archive_leaves_the_old_database() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GeoPaths::new(dir.path().to_path_buf(), "");
        std::fs::write(paths.default_xz(), b"not an archive").unwrap();
        std::fs::write(paths.default_db(), b"old").unwrap();
        assert!(matches!(decompress(&paths, |_| {}), Err(DownloadError::Decompress(_))));
        assert_eq!(std::fs::read(paths.default_db()).unwrap(), b"old");
        assert!(!dir.path().join("GeoLite2-City.mmdb.tmp").exists());
    }
}
