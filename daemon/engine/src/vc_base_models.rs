// Base RVC model download — RMVPE (pitch estimation) and ContentVec (feature
// extraction), fetched from the project's own model release rather than
// HuggingFace, with SHA-256 verification.
//
// Direct port of `voice_changer/rvc/model_downloader.py`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const BASE_URL: &str =
    "https://github.com/elegos/Linux-Arctis-Manager-AI-Models/releases/download/v1";

/// Legacy filename ContentVec was stored under in older installations.
const CONTENTVEC_LEGACY_FILENAME: &str = "contentvec_500.bin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    pub filename: &'static str,
    pub sha256: &'static str,
}

impl ModelSpec {
    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.filename)
    }

    pub fn path(&self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.filename)
    }
}

pub const RMVPE: ModelSpec = ModelSpec {
    filename: "rmvpe.pt",
    sha256: "6d62215f4306e3ca278246188607209f09af3dc77ed4232efdd069798c4ec193",
};

pub const CONTENTVEC: ModelSpec = ModelSpec {
    filename: "content_vec_best.bin",
    sha256: "d8dd400e054ddf4e6be75dab5a2549db748cc99e756a097c496c099f65a4854e",
};

pub fn base_models_dir(settings_base_dir: &Path) -> PathBuf {
    settings_base_dir.join("models")
}

pub fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Local path for `spec` if present on disk (checks the ContentVec legacy
/// filename too), `None` otherwise.
pub fn model_path(models_dir: &Path, spec: &ModelSpec) -> Option<PathBuf> {
    let p = spec.path(models_dir);
    if p.exists() {
        return Some(p);
    }
    if spec.filename == CONTENTVEC.filename {
        let legacy = models_dir.join(CONTENTVEC_LEGACY_FILENAME);
        if legacy.exists() {
            return Some(legacy);
        }
    }
    None
}

/// `(rmvpe_present, contentvec_present)`.
pub fn base_models_status(models_dir: &Path) -> (bool, bool) {
    (
        model_path(models_dir, &RMVPE).is_some(),
        model_path(models_dir, &CONTENTVEC).is_some(),
    )
}

#[derive(Debug)]
pub enum BaseModelError {
    Http(String),
    Io(String),
    ChecksumMismatch {
        filename: &'static str,
        expected: &'static str,
        actual: String,
    },
}

impl std::fmt::Display for BaseModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaseModelError::Http(msg) => write!(f, "download failed: {msg}"),
            BaseModelError::Io(msg) => write!(f, "filesystem error: {msg}"),
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

async fn download_file(spec: &ModelSpec, models_dir: &Path) -> Result<(), BaseModelError> {
    std::fs::create_dir_all(models_dir).map_err(|e| BaseModelError::Io(e.to_string()))?;
    let tmp = spec.path(models_dir).with_extension("tmp");

    let result: Result<(), BaseModelError> = async {
        let resp = reqwest::get(spec.url())
            .await
            .map_err(|e| BaseModelError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(BaseModelError::Http(format!("HTTP {}", resp.status())));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| BaseModelError::Http(e.to_string()))?;
        std::fs::write(&tmp, &bytes).map_err(|e| BaseModelError::Io(e.to_string()))?;

        let actual = sha256_hex(&tmp).map_err(|e| BaseModelError::Io(e.to_string()))?;
        if actual != spec.sha256 {
            return Err(BaseModelError::ChecksumMismatch {
                filename: spec.filename,
                expected: spec.sha256,
                actual,
            });
        }

        std::fs::rename(&tmp, spec.path(models_dir)).map_err(|e| BaseModelError::Io(e.to_string()))
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Download RMVPE and ContentVec from the official release, verifying
/// SHA-256. Already-present models are skipped.
pub async fn download_base_models(models_dir: &Path) -> Result<(), BaseModelError> {
    for spec in [&RMVPE, &CONTENTVEC] {
        if model_path(models_dir, spec).is_some() {
            continue;
        }
        download_file(spec, models_dir).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
    fn model_spec_url_and_path() {
        assert_eq!(
            RMVPE.url(),
            "https://github.com/elegos/Linux-Arctis-Manager-AI-Models/releases/download/v1/rmvpe.pt"
        );
        let dir = Path::new("/tmp/models");
        assert_eq!(RMVPE.path(dir), dir.join("rmvpe.pt"));
    }

    #[test]
    fn model_path_none_when_absent() {
        let dir = tempdir().unwrap();
        assert!(model_path(dir.path(), &RMVPE).is_none());
    }

    #[test]
    fn model_path_found_at_current_filename() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(RMVPE.filename), b"x").unwrap();
        assert_eq!(
            model_path(dir.path(), &RMVPE),
            Some(dir.path().join(RMVPE.filename))
        );
    }

    #[test]
    fn model_path_falls_back_to_contentvec_legacy_filename() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(CONTENTVEC_LEGACY_FILENAME), b"x").unwrap();
        assert_eq!(
            model_path(dir.path(), &CONTENTVEC),
            Some(dir.path().join(CONTENTVEC_LEGACY_FILENAME))
        );
    }

    #[test]
    fn base_models_status_reflects_presence() {
        let dir = tempdir().unwrap();
        assert_eq!(base_models_status(dir.path()), (false, false));
        std::fs::write(dir.path().join(RMVPE.filename), b"x").unwrap();
        assert_eq!(base_models_status(dir.path()), (true, false));
    }
}
