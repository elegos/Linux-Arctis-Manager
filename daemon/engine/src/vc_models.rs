// Local RVC model management — scan, lookup, delete.
//
// Direct port of `voice_changer/rvc/model_manager.py` (`RVCModelManager`).
// Models live in `<settings_base_dir>/rvc_models/`, one `.pth` file per
// model, optionally paired with a `.index` FAISS feature-retrieval file.

use std::path::{Path, PathBuf};

use serde::Serialize;

pub fn models_dir(base: &Path) -> PathBuf {
    base.join("rvc_models")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RvcModel {
    /// Display name — stem of the `.pth` file.
    pub name: String,
    pub path: PathBuf,
    /// True when a matching FAISS `.index` file sits next to the model.
    pub has_index: bool,
    /// `<stem>.onnx` next to the `.pth`, if `export_onnx.py` has been run
    /// for this model — `None` means the model exists locally but can't be
    /// rendered/converted yet (the live chain and calibration both need the
    /// exported graph, not the `.pth` checkpoint).
    pub onnx_path: Option<PathBuf>,
}

/// Same matching rule as the RVC pipeline: `<stem>.index`, or any `*.index`
/// whose filename contains the model stem (RVC WebUI exports
/// `added_IVF…_<name>_v2.index`).
pub fn find_index_path(pth: &Path) -> Option<PathBuf> {
    let stem = pth.file_stem().and_then(|s| s.to_str())?;
    let exact = pth.with_file_name(format!("{stem}.index"));
    if exact.is_file() {
        return Some(exact);
    }
    let parent = pth.parent()?;
    std::fs::read_dir(parent)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|e| e == "index")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.contains(stem))
        })
}

pub fn index_exists(pth: &Path) -> bool {
    find_index_path(pth).is_some()
}

/// Scan `<base>/rvc_models/` for `.pth` files (non-recursive), sorted by name.
/// Returns an empty list if the folder does not exist.
pub fn list_models(base: &Path) -> Vec<RvcModel> {
    let dir = models_dir(base);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut models: Vec<RvcModel> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "pth"))
        .map(|p| {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            let has_index = index_exists(&p);
            let onnx_path = p.with_extension("onnx");
            let onnx_path = onnx_path.is_file().then_some(onnx_path);
            RvcModel {
                name,
                path: p,
                has_index,
                onnx_path,
            }
        })
        .collect();
    models.sort_by(|a, b| a.name.cmp(&b.name));
    models
}

pub fn find_model(base: &Path, name: &str) -> Option<RvcModel> {
    list_models(base).into_iter().find(|m| m.name == name)
}

/// Delete a local model's `.pth` file by stem name. Returns false if it does
/// not exist. Matches the Python reference: the paired `.index` file (if
/// any) is left in place, not deleted.
pub fn delete_model(base: &Path, name: &str) -> bool {
    let path = models_dir(base).join(format!("{name}.pth"));
    path.is_file() && std::fs::remove_file(&path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn touch(path: &Path) {
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn list_models_empty_when_folder_missing() {
        let base = tempdir().unwrap();
        assert!(list_models(base.path()).is_empty());
    }

    #[test]
    fn list_models_finds_pth_files_only() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        touch(&dir.join("voice_a.pth"));
        touch(&dir.join("readme.txt"));
        touch(&dir.join("voice_a.index"));

        let models = list_models(base.path());
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "voice_a");
        assert!(models[0].has_index);
    }

    #[test]
    fn list_models_detects_exported_onnx() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        touch(&dir.join("voice_a.pth"));
        touch(&dir.join("voice_a.onnx"));
        touch(&dir.join("voice_b.pth")); // no .onnx: not exported yet

        let models = list_models(base.path());
        let a = models.iter().find(|m| m.name == "voice_a").unwrap();
        let b = models.iter().find(|m| m.name == "voice_b").unwrap();
        assert_eq!(a.onnx_path, Some(dir.join("voice_a.onnx")));
        assert_eq!(b.onnx_path, None);
    }

    #[test]
    fn list_models_sorted_by_name() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        touch(&dir.join("zeta.pth"));
        touch(&dir.join("alpha.pth"));

        let models = list_models(base.path());
        assert_eq!(models[0].name, "alpha");
        assert_eq!(models[1].name, "zeta");
    }

    #[test]
    fn index_exists_matches_exact_stem() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        let pth = dir.join("my_voice.pth");
        touch(&pth);
        touch(&dir.join("my_voice.index"));
        assert!(index_exists(&pth));
    }

    #[test]
    fn index_exists_matches_prefixed_ivf_export() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        let pth = dir.join("my_voice.pth");
        touch(&pth);
        touch(&dir.join("added_IVF256_my_voice_v2.index"));
        assert!(index_exists(&pth));
    }

    #[test]
    fn index_exists_false_when_no_match() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        let pth = dir.join("my_voice.pth");
        touch(&pth);
        touch(&dir.join("other_voice.index"));
        assert!(!index_exists(&pth));
    }

    #[test]
    fn find_model_returns_none_when_absent() {
        let base = tempdir().unwrap();
        assert!(find_model(base.path(), "missing").is_none());
    }

    #[test]
    fn find_model_returns_match_by_name() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        touch(&dir.join("voice_a.pth"));
        let found = find_model(base.path(), "voice_a");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "voice_a");
    }

    #[test]
    fn delete_model_removes_pth_but_keeps_index() {
        let base = tempdir().unwrap();
        let dir = models_dir(base.path());
        std::fs::create_dir_all(&dir).unwrap();
        let pth = dir.join("voice_a.pth");
        let idx = dir.join("voice_a.index");
        touch(&pth);
        touch(&idx);

        assert!(delete_model(base.path(), "voice_a"));
        assert!(!pth.exists());
        assert!(idx.exists(), "the paired .index file must be left in place");
    }

    #[test]
    fn delete_model_returns_false_when_absent() {
        let base = tempdir().unwrap();
        assert!(!delete_model(base.path(), "missing"));
    }
}
