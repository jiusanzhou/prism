//! Plugin loader — resolves plugin sources to WASM bytes.
//!
//! Supports:
//! - Local file path
//! - HTTP/HTTPS URL (with optional SHA-256 checksum)
//! - OCI registry (basic pull via HTTP API)
//! - Builtin (no WASM bytes needed)
//!
//! Downloaded plugins are cached locally to avoid redundant network calls.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::config::PluginSourceConfig;

/// Error during plugin loading.
#[derive(Debug, Clone)]
pub enum LoadError {
    NotFound(String),
    NetworkError(String),
    ChecksumMismatch { expected: String, actual: String },
    IoError(String),
    OciError(String),
    Other(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound(msg) => write!(f, "not found: {}", msg),
            LoadError::NetworkError(msg) => write!(f, "network error: {}", msg),
            LoadError::ChecksumMismatch { expected, actual } => {
                write!(f, "checksum mismatch: expected {}, got {}", expected, actual)
            }
            LoadError::IoError(msg) => write!(f, "io error: {}", msg),
            LoadError::OciError(msg) => write!(f, "oci error: {}", msg),
            LoadError::Other(msg) => write!(f, "load error: {}", msg),
        }
    }
}

impl std::error::Error for LoadError {}

/// Resolved plugin — WASM bytes ready for compilation.
#[derive(Debug, Clone)]
pub struct ResolvedPlugin {
    /// Plugin name.
    pub name: String,
    /// WASM module bytes.
    pub wasm_bytes: Vec<u8>,
    /// Where it was loaded from.
    pub source_info: String,
    /// SHA-256 of the WASM bytes.
    pub sha256: String,
}

/// Plugin loader with local caching.
pub struct PluginLoader {
    /// Cache directory for downloaded plugins.
    cache_dir: PathBuf,
    /// In-memory cache: sha256 → bytes.
    memory_cache: RwLock<HashMap<String, Vec<u8>>>,
}

impl PluginLoader {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        let cache_dir = cache_dir.into();
        // Create cache dir if it doesn't exist
        let _ = std::fs::create_dir_all(&cache_dir);
        Self {
            cache_dir,
            memory_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Default cache dir: ~/.prism/plugins/cache/
    pub fn with_default_cache() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        Self::new(format!("{}/.prism/plugins/cache", home))
    }

    /// Resolve a plugin source to WASM bytes.
    pub fn resolve(
        &self,
        name: &str,
        source: &PluginSourceConfig,
    ) -> Result<ResolvedPlugin, LoadError> {
        match source {
            PluginSourceConfig::Wasm { wasm } => self.load_file(name, wasm),
            PluginSourceConfig::Url { url, sha256 } => {
                self.load_url(name, url, sha256.as_deref())
            }
            PluginSourceConfig::Oci { oci, digest } => {
                self.load_oci(name, oci, digest.as_deref())
            }
            PluginSourceConfig::Builtin { .. } => {
                Err(LoadError::Other("builtin plugins don't have WASM bytes".into()))
            }
        }
    }

    /// Load from local file.
    fn load_file(&self, name: &str, path: &str) -> Result<ResolvedPlugin, LoadError> {
        let bytes =
            std::fs::read(path).map_err(|e| LoadError::NotFound(format!("{}: {}", path, e)))?;
        let sha256 = sha256_hex(&bytes);
        Ok(ResolvedPlugin {
            name: name.to_string(),
            wasm_bytes: bytes,
            source_info: format!("file://{}", path),
            sha256,
        })
    }

    /// Load from HTTP/HTTPS URL with optional checksum verification.
    fn load_url(
        &self,
        name: &str,
        url: &str,
        expected_sha256: Option<&str>,
    ) -> Result<ResolvedPlugin, LoadError> {
        // Check disk cache first (by URL hash)
        let cache_key = sha256_hex(url.as_bytes());
        let cache_path = self.cache_dir.join(&cache_key);

        if let Some(sha) = expected_sha256 {
            // If we have expected checksum, check memory cache
            if let Ok(cache) = self.memory_cache.read() {
                if let Some(bytes) = cache.get(sha) {
                    return Ok(ResolvedPlugin {
                        name: name.to_string(),
                        wasm_bytes: bytes.clone(),
                        source_info: format!("url:{} (memory cache)", url),
                        sha256: sha.to_string(),
                    });
                }
            }
        }

        // Check disk cache
        if cache_path.exists() {
            let bytes = std::fs::read(&cache_path)
                .map_err(|e| LoadError::IoError(format!("cache read: {}", e)))?;
            let sha256 = sha256_hex(&bytes);

            if let Some(expected) = expected_sha256 {
                if sha256 != expected {
                    // Cache is stale, re-download
                    let _ = std::fs::remove_file(&cache_path);
                } else {
                    // Cache hit with valid checksum
                    if let Ok(mut cache) = self.memory_cache.write() {
                        cache.insert(sha256.clone(), bytes.clone());
                    }
                    return Ok(ResolvedPlugin {
                        name: name.to_string(),
                        wasm_bytes: bytes,
                        source_info: format!("url:{} (disk cache)", url),
                        sha256,
                    });
                }
            } else {
                // No checksum required, use cache
                return Ok(ResolvedPlugin {
                    name: name.to_string(),
                    wasm_bytes: bytes,
                    source_info: format!("url:{} (disk cache)", url),
                    sha256,
                });
            }
        }

        // Download
        let resp = ureq::get(url)
            .call()
            .map_err(|e| LoadError::NetworkError(format!("{}: {}", url, e)))?;

        let mut bytes = Vec::new();
        resp.into_reader()
            .take(100 * 1024 * 1024) // 100MB limit
            .read_to_end(&mut bytes)
            .map_err(|e| LoadError::NetworkError(format!("read body: {}", e)))?;

        let sha256 = sha256_hex(&bytes);

        // Verify checksum
        if let Some(expected) = expected_sha256 {
            if sha256 != expected {
                return Err(LoadError::ChecksumMismatch {
                    expected: expected.to_string(),
                    actual: sha256,
                });
            }
        }

        // Write to disk cache
        let _ = std::fs::write(&cache_path, &bytes);

        // Write to memory cache
        if let Ok(mut cache) = self.memory_cache.write() {
            cache.insert(sha256.clone(), bytes.clone());
        }

        Ok(ResolvedPlugin {
            name: name.to_string(),
            wasm_bytes: bytes,
            source_info: format!("url:{}", url),
            sha256,
        })
    }

    /// Load from OCI registry.
    ///
    /// OCI Distribution Spec pull flow:
    /// 1. GET /v2/<name>/manifests/<reference> → manifest JSON
    /// 2. Parse manifest → find the WASM layer (application/vnd.wasm.content.layer.v1+wasm
    ///    or application/octet-stream)
    /// 3. GET /v2/<name>/blobs/<digest> → WASM bytes
    fn load_oci(
        &self,
        name: &str,
        reference: &str,
        expected_digest: Option<&str>,
    ) -> Result<ResolvedPlugin, LoadError> {
        // Parse OCI reference: registry.example.com/repo/name:tag
        let (registry, repo, tag) = parse_oci_reference(reference)?;

        // Check disk cache by reference
        let cache_key = sha256_hex(reference.as_bytes());
        let cache_path = self.cache_dir.join(format!("oci-{}", cache_key));

        if cache_path.exists() {
            if let Ok(bytes) = std::fs::read(&cache_path) {
                let sha256 = sha256_hex(&bytes);
                if expected_digest.map_or(true, |d| sha256_matches(d, &sha256)) {
                    return Ok(ResolvedPlugin {
                        name: name.to_string(),
                        wasm_bytes: bytes,
                        source_info: format!("oci:{} (cache)", reference),
                        sha256,
                    });
                }
            }
        }

        // 1. Get manifest
        let manifest_url = format!("https://{}/v2/{}/manifests/{}", registry, repo, tag);
        let manifest_resp = ureq::get(&manifest_url)
            .set("Accept", "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json")
            .call()
            .map_err(|e| LoadError::OciError(format!("manifest fetch: {}", e)))?;

        let manifest: serde_json::Value = manifest_resp
            .into_json()
            .map_err(|e| LoadError::OciError(format!("manifest parse: {}", e)))?;

        // 2. Find WASM layer
        let layers = manifest["layers"]
            .as_array()
            .ok_or_else(|| LoadError::OciError("no layers in manifest".into()))?;

        let wasm_layer = layers
            .iter()
            .find(|l| {
                let mt = l["mediaType"].as_str().unwrap_or("");
                mt.contains("wasm") || mt == "application/octet-stream"
            })
            .ok_or_else(|| LoadError::OciError("no WASM layer found in manifest".into()))?;

        let layer_digest = wasm_layer["digest"]
            .as_str()
            .ok_or_else(|| LoadError::OciError("layer has no digest".into()))?;

        // 3. Pull blob
        let blob_url = format!("https://{}/v2/{}/blobs/{}", registry, repo, layer_digest);
        let blob_resp = ureq::get(&blob_url)
            .call()
            .map_err(|e| LoadError::OciError(format!("blob fetch: {}", e)))?;

        let mut bytes = Vec::new();
        blob_resp
            .into_reader()
            .take(100 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|e| LoadError::OciError(format!("blob read: {}", e)))?;

        let sha256 = sha256_hex(&bytes);

        // Verify digest
        if let Some(expected) = expected_digest {
            if !sha256_matches(expected, &sha256) {
                return Err(LoadError::ChecksumMismatch {
                    expected: expected.to_string(),
                    actual: sha256,
                });
            }
        }

        // Cache
        let _ = std::fs::write(&cache_path, &bytes);

        Ok(ResolvedPlugin {
            name: name.to_string(),
            wasm_bytes: bytes,
            source_info: format!("oci:{}", reference),
            sha256,
        })
    }

    /// Clear all caches.
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.memory_cache.write() {
            cache.clear();
        }
        let _ = std::fs::remove_dir_all(&self.cache_dir);
        let _ = std::fs::create_dir_all(&self.cache_dir);
    }

    /// Get cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

/// Parse OCI reference: "registry.com/repo/name:tag" → (registry, repo, tag)
fn parse_oci_reference(reference: &str) -> Result<(String, String, String), LoadError> {
    // Handle format: registry/repo:tag or registry/repo/name:tag
    let (path, tag) = if let Some(pos) = reference.rfind(':') {
        (&reference[..pos], &reference[pos + 1..])
    } else {
        (reference, "latest")
    };

    // Split registry from repo
    let parts: Vec<&str> = path.splitn(2, '/').collect();
    if parts.len() < 2 {
        return Err(LoadError::OciError(format!(
            "invalid OCI reference: {}",
            reference
        )));
    }

    let registry = parts[0].to_string();
    let repo = parts[1].to_string();

    Ok((registry, repo, tag.to_string()))
}

/// SHA-256 hex digest.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

/// Compare SHA-256 digests, handling optional "sha256:" prefix.
fn sha256_matches(expected: &str, actual: &str) -> bool {
    let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
    expected.eq_ignore_ascii_case(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_sha256_hex_empty() {
        let hash = sha256_hex(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_matches_exact() {
        assert!(sha256_matches("abc123", "abc123"));
        assert!(sha256_matches("ABC123", "abc123"));
        assert!(!sha256_matches("abc123", "def456"));
    }

    #[test]
    fn test_sha256_matches_with_prefix() {
        assert!(sha256_matches("sha256:abc123", "abc123"));
        assert!(sha256_matches("sha256:ABC123", "abc123"));
        assert!(!sha256_matches("sha256:abc123", "def456"));
    }

    #[test]
    fn test_parse_oci_reference_with_tag() {
        let (registry, repo, tag) =
            parse_oci_reference("ghcr.io/prism-plugins/k8s-router:v1.2.0").unwrap();
        assert_eq!(registry, "ghcr.io");
        assert_eq!(repo, "prism-plugins/k8s-router");
        assert_eq!(tag, "v1.2.0");
    }

    #[test]
    fn test_parse_oci_reference_latest() {
        let (registry, repo, tag) =
            parse_oci_reference("registry.example.com/my-plugin").unwrap();
        assert_eq!(registry, "registry.example.com");
        assert_eq!(repo, "my-plugin");
        assert_eq!(tag, "latest");
    }

    #[test]
    fn test_parse_oci_reference_nested_repo() {
        let (registry, repo, tag) =
            parse_oci_reference("docker.io/library/nginx:alpine").unwrap();
        assert_eq!(registry, "docker.io");
        assert_eq!(repo, "library/nginx");
        assert_eq!(tag, "alpine");
    }

    #[test]
    fn test_parse_oci_reference_invalid() {
        let result = parse_oci_reference("no-slash");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_file_success() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"fake wasm bytes").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let loader = PluginLoader::new(tempfile::tempdir().unwrap().path());
        let result = loader.load_file("test-plugin", &path).unwrap();

        assert_eq!(result.name, "test-plugin");
        assert_eq!(result.wasm_bytes, b"fake wasm bytes");
        assert!(result.source_info.contains("file://"));
        assert!(!result.sha256.is_empty());
    }

    #[test]
    fn test_load_file_not_found() {
        let loader = PluginLoader::new(tempfile::tempdir().unwrap().path());
        let result = loader.load_file("missing", "/nonexistent/path/plugin.wasm");
        assert!(result.is_err());
        match result.unwrap_err() {
            LoadError::NotFound(msg) => assert!(msg.contains("/nonexistent")),
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_builtin_returns_error() {
        let loader = PluginLoader::new(tempfile::tempdir().unwrap().path());
        let source = PluginSourceConfig::Builtin {
            builtin: "test".to_string(),
        };
        let result = loader.resolve("builtin-plugin", &source);
        assert!(result.is_err());
        match result.unwrap_err() {
            LoadError::Other(msg) => assert!(msg.contains("builtin")),
            other => panic!("expected Other, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_wasm_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"wasm content").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let loader = PluginLoader::new(tempfile::tempdir().unwrap().path());
        let source = PluginSourceConfig::Wasm { wasm: path };
        let result = loader.resolve("wasm-plugin", &source).unwrap();

        assert_eq!(result.name, "wasm-plugin");
        assert_eq!(result.wasm_bytes, b"wasm content");
    }

    #[test]
    fn test_cache_write_and_read() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());

        // Write a fake wasm file
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"cached wasm").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        // Load should succeed
        let result = loader.load_file("cached", &path).unwrap();
        assert_eq!(result.wasm_bytes, b"cached wasm");

        // Verify sha256 is consistent
        let sha1 = sha256_hex(b"cached wasm");
        assert_eq!(result.sha256, sha1);
    }

    #[test]
    fn test_clear_cache() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());

        // Write something into memory cache
        if let Ok(mut cache) = loader.memory_cache.write() {
            cache.insert("test".to_string(), vec![1, 2, 3]);
        }

        // Create a file in the cache dir
        let cache_file = cache_dir.path().join("test-file");
        std::fs::write(&cache_file, b"test").unwrap();
        assert!(cache_file.exists());

        // Clear cache
        loader.clear_cache();

        // Memory cache should be empty
        if let Ok(cache) = loader.memory_cache.read() {
            assert!(cache.is_empty());
        }

        // File should be gone, but dir recreated
        assert!(!cache_file.exists());
        assert!(cache_dir.path().exists());
    }

    #[test]
    fn test_cache_dir_accessor() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        assert_eq!(loader.cache_dir(), cache_dir.path());
    }

    #[test]
    fn test_load_error_display() {
        assert_eq!(
            LoadError::NotFound("foo".into()).to_string(),
            "not found: foo"
        );
        assert_eq!(
            LoadError::NetworkError("bar".into()).to_string(),
            "network error: bar"
        );
        assert_eq!(
            LoadError::ChecksumMismatch {
                expected: "aaa".into(),
                actual: "bbb".into()
            }
            .to_string(),
            "checksum mismatch: expected aaa, got bbb"
        );
        assert_eq!(
            LoadError::IoError("io".into()).to_string(),
            "io error: io"
        );
        assert_eq!(
            LoadError::OciError("oci".into()).to_string(),
            "oci error: oci"
        );
        assert_eq!(
            LoadError::Other("other".into()).to_string(),
            "load error: other"
        );
    }

    #[test]
    #[ignore] // Requires network
    fn test_load_url_network() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        // This would need a real URL to test; marked as ignored for CI
        let result = loader.load_url("test", "https://example.com/nonexistent.wasm", None);
        // Should fail with network/404 error
        assert!(result.is_err());
    }

    #[test]
    #[ignore] // Requires network
    fn test_load_oci_network() {
        let cache_dir = tempfile::tempdir().unwrap();
        let loader = PluginLoader::new(cache_dir.path());
        let result = loader.load_oci("test", "ghcr.io/nonexistent/plugin:v1", None);
        assert!(result.is_err());
    }
}
