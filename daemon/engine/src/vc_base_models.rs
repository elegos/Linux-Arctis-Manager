// Base RVC model download — RMVPE (pitch estimation) and ContentVec (feature
// extraction), as ONNX. Version and checksums are resolved dynamically from
// the project's own model release on GitHub — nothing about the release is
// hardcoded here beyond the two filenames this daemon actually needs, so a
// re-export (new checksum, e.g. after re-converting for a newer opset) only
// needs a new GitHub release, not a daemon rebuild.
//
// `elegos/Linux-Arctis-Manager-AI-Models`'s *latest* (non-prerelease) GitHub
// release is the single source of truth: its `checksum.onnx.sha256` asset
// (a standard `sha256sum`-format manifest) gives the expected hash for each
// named file, and the release's own asset list gives the download URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const RELEASES_API_URL: &str =
    "https://api.github.com/repos/elegos/Linux-Arctis-Manager-AI-Models/releases/latest";
const MANIFEST_ASSET_NAME: &str = "checksum.onnx.sha256";

pub const RMVPE_FILENAME: &str = "rmvpe.onnx";
pub const CONTENTVEC_FILENAME: &str = "content_vec_best.onnx";

pub fn base_models_dir(settings_base_dir: &Path) -> PathBuf {
    settings_base_dir.join("models")
}

pub fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn model_path(models_dir: &Path, filename: &str) -> Option<PathBuf> {
    let p = models_dir.join(filename);
    p.exists().then_some(p)
}

/// `(rmvpe_present, contentvec_present)`. Purely local — no network call.
pub fn base_models_status(models_dir: &Path) -> (bool, bool) {
    (
        model_path(models_dir, RMVPE_FILENAME).is_some(),
        model_path(models_dir, CONTENTVEC_FILENAME).is_some(),
    )
}

#[derive(Debug)]
pub enum BaseModelError {
    Http(String),
    Io(String),
    /// `checksum.onnx.sha256` doesn't list this filename.
    ManifestMissingEntry(String),
    /// The release has no asset with this exact filename.
    AssetNotFound(String),
    ChecksumMismatch {
        filename: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for BaseModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaseModelError::Http(msg) => write!(f, "download failed: {msg}"),
            BaseModelError::Io(msg) => write!(f, "filesystem error: {msg}"),
            BaseModelError::ManifestMissingEntry(name) => {
                write!(f, "{MANIFEST_ASSET_NAME} does not list {name}")
            }
            BaseModelError::AssetNotFound(name) => {
                write!(f, "latest release has no asset named {name}")
            }
            BaseModelError::ChecksumMismatch {
                filename,
                expected,
                actual,
            } => write!(
                f,
                "checksum mismatch for {filename}: expected {expected}, got {actual}"
            ),
        }
    }
}

// ── GitHub API ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    assets: Vec<GhAsset>,
}

fn github_client() -> reqwest::Client {
    // GitHub's API rejects requests with no User-Agent.
    reqwest::Client::builder()
        .user_agent(concat!("linux-arctis-manager/", env!("LAM_VERSION")))
        .build()
        .expect("reqwest client builder should not fail with default settings")
}

async fn fetch_latest_release() -> Result<GhRelease, BaseModelError> {
    let resp = github_client()
        .get(RELEASES_API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| BaseModelError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(BaseModelError::Http(format!(
            "GitHub API HTTP {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| BaseModelError::Http(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| BaseModelError::Http(e.to_string()))
}

async fn get_bytes(url: &str) -> Result<Vec<u8>, BaseModelError> {
    let resp = github_client()
        .get(url)
        .send()
        .await
        .map_err(|e| BaseModelError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(BaseModelError::Http(format!("HTTP {}", resp.status())));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| BaseModelError::Http(e.to_string()))
}

/// Parse a `sha256sum`-format manifest (`<hash>  <filename>` per line, an
/// optional `*` before the filename for binary mode) into filename -> lowercase
/// hex hash. Blank lines are skipped; malformed lines are silently ignored
/// rather than failing the whole manifest.
fn parse_manifest(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(2, char::is_whitespace);
            let hash = parts.next()?.trim().to_lowercase();
            let filename = parts.next()?.trim().trim_start_matches('*').to_owned();
            if hash.is_empty() || filename.is_empty() {
                return None;
            }
            Some((filename, hash))
        })
        .collect()
}

async fn download_named_model(
    release: &GhRelease,
    manifest: &HashMap<String, String>,
    filename: &str,
    models_dir: &Path,
) -> Result<(), BaseModelError> {
    let expected_sha = manifest
        .get(filename)
        .ok_or_else(|| BaseModelError::ManifestMissingEntry(filename.to_owned()))?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == filename)
        .ok_or_else(|| BaseModelError::AssetNotFound(filename.to_owned()))?;

    let tmp = models_dir.join(format!("{filename}.tmp"));
    let result: Result<(), BaseModelError> = async {
        let bytes = get_bytes(&asset.browser_download_url).await?;
        std::fs::write(&tmp, &bytes).map_err(|e| BaseModelError::Io(e.to_string()))?;

        let actual = sha256_hex(&tmp).map_err(|e| BaseModelError::Io(e.to_string()))?;
        if &actual != expected_sha {
            return Err(BaseModelError::ChecksumMismatch {
                filename: filename.to_owned(),
                expected: expected_sha.clone(),
                actual,
            });
        }

        std::fs::rename(&tmp, models_dir.join(filename))
            .map_err(|e| BaseModelError::Io(e.to_string()))
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Download RMVPE and ContentVec (ONNX) from the latest release of
/// `elegos/Linux-Arctis-Manager-AI-Models`, verifying SHA-256 against that
/// same release's `checksum.onnx.sha256`. Already-present models are skipped
/// without any network call at all.
pub async fn download_base_models(models_dir: &Path) -> Result<(), BaseModelError> {
    let (rmvpe_ok, contentvec_ok) = base_models_status(models_dir);
    if rmvpe_ok && contentvec_ok {
        return Ok(());
    }

    std::fs::create_dir_all(models_dir).map_err(|e| BaseModelError::Io(e.to_string()))?;

    let release = fetch_latest_release().await?;
    let manifest_asset = release
        .assets
        .iter()
        .find(|a| a.name == MANIFEST_ASSET_NAME)
        .ok_or_else(|| BaseModelError::AssetNotFound(MANIFEST_ASSET_NAME.to_owned()))?;
    let manifest_text = String::from_utf8(get_bytes(&manifest_asset.browser_download_url).await?)
        .map_err(|e| BaseModelError::Http(e.to_string()))?;
    let manifest = parse_manifest(&manifest_text);

    if !rmvpe_ok {
        download_named_model(&release, &manifest, RMVPE_FILENAME, models_dir).await?;
    }
    if !contentvec_ok {
        download_named_model(&release, &manifest, CONTENTVEC_FILENAME, models_dir).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Not run by default (`cargo test`) — real network call against the
    /// live `elegos/Linux-Arctis-Manager-AI-Models` repo. Run manually with
    /// `cargo test --bin lam-daemon -- --ignored live_latest_release_resolves_known_models`
    /// after publishing a new release, to sanity-check resolution end to end
    /// without downloading the full multi-hundred-MB model files.
    #[tokio::test]
    #[ignore]
    async fn live_latest_release_resolves_known_models() {
        let release = fetch_latest_release().await.expect("fetch latest release");
        let manifest_asset = release
            .assets
            .iter()
            .find(|a| a.name == MANIFEST_ASSET_NAME)
            .expect("release has checksum.onnx.sha256");
        let manifest_text = String::from_utf8(
            get_bytes(&manifest_asset.browser_download_url)
                .await
                .unwrap(),
        )
        .unwrap();
        let manifest = parse_manifest(&manifest_text);

        for filename in [RMVPE_FILENAME, CONTENTVEC_FILENAME] {
            let hash = manifest
                .get(filename)
                .unwrap_or_else(|| panic!("{filename} missing from manifest"));
            assert_eq!(
                hash.len(),
                64,
                "{filename}: hash isn't 64 hex chars: {hash}"
            );
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "{filename}: hash isn't hex: {hash}"
            );
            assert!(
                release.assets.iter().any(|a| a.name == filename),
                "{filename} listed in manifest but no matching release asset"
            );
        }
    }

    #[test]
    fn sha256_hex_of_empty_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            sha256_hex(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_of_known_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hello");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(
            sha256_hex(&path).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn model_path_none_when_absent() {
        let dir = tempdir().unwrap();
        assert!(model_path(dir.path(), RMVPE_FILENAME).is_none());
    }

    #[test]
    fn model_path_found_at_known_filename() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(RMVPE_FILENAME), b"x").unwrap();
        assert_eq!(
            model_path(dir.path(), RMVPE_FILENAME),
            Some(dir.path().join(RMVPE_FILENAME))
        );
    }

    #[test]
    fn base_models_status_reflects_presence() {
        let dir = tempdir().unwrap();
        assert_eq!(base_models_status(dir.path()), (false, false));
        std::fs::write(dir.path().join(RMVPE_FILENAME), b"x").unwrap();
        assert_eq!(base_models_status(dir.path()), (true, false));
        std::fs::write(dir.path().join(CONTENTVEC_FILENAME), b"x").unwrap();
        assert_eq!(base_models_status(dir.path()), (true, true));
    }

    // ── parse_manifest ───────────────────────────────────────────────────

    #[test]
    fn parse_manifest_standard_sha256sum_format() {
        let text = "d8dd400e054ddf4e6be75dab5a2549db748cc99e756a097c496c099f65a4854e  content_vec_best.onnx\n\
                     6d62215f4306e3ca278246188607209f09af3dc77ed4232efdd069798c4ec193  rmvpe.onnx\n";
        let m = parse_manifest(text);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m.get("rmvpe.onnx").map(String::as_str),
            Some("6d62215f4306e3ca278246188607209f09af3dc77ed4232efdd069798c4ec193")
        );
        assert_eq!(
            m.get("content_vec_best.onnx").map(String::as_str),
            Some("d8dd400e054ddf4e6be75dab5a2549db748cc99e756a097c496c099f65a4854e")
        );
    }

    #[test]
    fn parse_manifest_accepts_binary_mode_asterisk_and_single_space() {
        let text = "aabbcc *rmvpe.onnx\n";
        let m = parse_manifest(text);
        assert_eq!(m.get("rmvpe.onnx").map(String::as_str), Some("aabbcc"));
    }

    #[test]
    fn parse_manifest_uppercases_normalised_to_lowercase() {
        let text = "AABBCC  model.onnx\n";
        let m = parse_manifest(text);
        assert_eq!(m.get("model.onnx").map(String::as_str), Some("aabbcc"));
    }

    #[test]
    fn parse_manifest_skips_blank_lines_and_trailing_newline() {
        let text = "\naabbcc  a.onnx\n\n\nddeeff  b.onnx\n\n";
        let m = parse_manifest(text);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn parse_manifest_empty_text_yields_empty_map() {
        assert!(parse_manifest("").is_empty());
    }

    #[test]
    fn parse_manifest_ignores_hash_only_line() {
        let text = "aabbcc\nddeeff  real.onnx\n";
        let m = parse_manifest(text);
        assert_eq!(m.len(), 1);
        assert!(m.contains_key("real.onnx"));
    }
}
