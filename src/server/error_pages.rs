use std::collections::HashMap;
use std::path::Path;

use bytes::Bytes;

/// Pre-loaded HTML error pages, keyed by status code.
/// Loaded once at startup — `HashMap::get()` on the hot path, no I/O.
pub struct ErrorPages {
    pages: HashMap<u16, Bytes>,
}

impl ErrorPages {
    /// Load error pages from a directory. Files must be named `{status}.html`
    /// (e.g., `404.html`, `500.html`).
    pub fn load(dir: &Path) -> Result<Self, crate::types::BoxError> {
        let mut pages = HashMap::new();

        let entries = std::fs::read_dir(dir)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("html") {
                continue;
            }

            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(status) = stem.parse::<u16>() {
                    if (400..=599).contains(&status) {
                        let content = std::fs::read(&path)?;
                        tracing::info!(status, path = %path.display(), "Loaded custom error page");
                        pages.insert(status, Bytes::from(content));
                    }
                }
            }
        }

        Ok(Self { pages })
    }

    /// Get a pre-loaded error page by status code.
    pub fn get(&self, status: u16) -> Option<Bytes> {
        self.pages.get(&status).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_error_pages() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("404.html"), "<h1>Not Found</h1>").unwrap();
        std::fs::write(dir.path().join("500.html"), "<h1>Error</h1>").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "ignored").unwrap();

        let pages = ErrorPages::load(dir.path()).unwrap();
        assert!(pages.get(404).is_some());
        assert!(pages.get(500).is_some());
        assert!(pages.get(403).is_none());
    }

    #[test]
    fn test_missing_dir() {
        let result = ErrorPages::load(Path::new("/nonexistent/dir"));
        assert!(result.is_err());
    }

    #[test]
    fn test_ignores_invalid_filenames() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("abc.html"), "not a status code").unwrap();
        std::fs::write(dir.path().join("200.html"), "not an error").unwrap();

        let pages = ErrorPages::load(dir.path()).unwrap();
        assert!(pages.get(200).is_none()); // 200 not in 400-599 range
    }
}
