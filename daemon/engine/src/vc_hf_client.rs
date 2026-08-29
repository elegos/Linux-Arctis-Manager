// HuggingFace Hub client — model search, repo file listing, and download.
//
// Direct port of `voice_changer/rvc/hf_search.py`, using the public HF Hub
// REST API (https://huggingface.co/docs/hub/api) via `reqwest` instead of
// the `huggingface_hub` Python SDK. `.pth` downloads and `.zip` archives
// (RVC WebUI sometimes bundles `.pth` + `.index` together) are both
// supported, each followed by a best-effort matching `.index` sidecar fetch.
//
// Not yet wired into dbus.rs — the `VcInterface` D-Bus service lands in a
// later phase ([E10-S5], see docs/voice-changing-feature.md). Unit tests
// below exercise the pure parts (URL building, JSON parsing, tie-break
// logic, zip extraction) directly in the meantime.
#![allow(dead_code)]

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;

const MODEL_EXTENSIONS: [&str; 2] = [".pth", ".zip"];

// ── Token management ────────────────────────────────────────────────────────

pub fn token_file(base: &Path) -> PathBuf {
    base.join("hf_token")
}

/// Resolve the HuggingFace token: the daemon's own token file first, then
/// `HF_TOKEN`, then the `huggingface-cli login` cache — so a token set up
/// via the Python HF SDK elsewhere on the system still works.
pub fn get_token(base: &Path) -> Option<String> {
    let own = read_trimmed(&token_file(base));
    let env = std::env::var("HF_TOKEN")
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    let cache = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cache/huggingface/token"))
        .and_then(|p| read_trimmed(&p));
    pick_token(own, env, cache)
}

/// Priority order: the daemon's own token file, then `HF_TOKEN`, then the
/// `huggingface-cli login` cache. Pure so the priority order is directly
/// testable without touching real environment/filesystem state.
fn pick_token(own: Option<String>, env: Option<String>, cache: Option<String>) -> Option<String> {
    own.or(env).or(cache)
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

pub fn set_token(base: &Path, token: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(base)?;
    let path = token_file(base);
    std::fs::write(&path, token.trim())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ── Search ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HfModelCard {
    pub repo_id: String,
    pub name: String,
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
}

fn valid_sort(sort_by: &str) -> &str {
    match sort_by {
        "downloads" | "likes" | "trendingScore" => sort_by,
        _ => "downloads",
    }
}

/// Build the `GET /api/models` search URL. A blank query browses the `rvc`
/// tag (matching the Python reference's "tag filter only when browsing,
/// named search is open" behaviour).
pub fn search_url(query: &str, sort_by: &str, limit: u32) -> reqwest::Url {
    let mut url = reqwest::Url::parse("https://huggingface.co/api/models")
        .expect("static HF API URL must parse");
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("sort", valid_sort(sort_by));
        qp.append_pair("direction", "-1");
        qp.append_pair("limit", &limit.to_string());
        let q = query.trim();
        if q.is_empty() {
            qp.append_pair("filter", "rvc");
        } else {
            qp.append_pair("search", q);
        }
    }
    url
}

pub fn parse_search_response(json: &str) -> Vec<HfModelCard> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    let Some(arr) = v.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|m| {
            let repo_id = m["id"].as_str()?.to_owned();
            let name = repo_id.rsplit('/').next().unwrap_or(&repo_id).to_owned();
            Some(HfModelCard {
                author: m["author"].as_str().unwrap_or("").to_owned(),
                downloads: m["downloads"].as_u64().unwrap_or(0),
                likes: m["likes"].as_u64().unwrap_or(0),
                name,
                repo_id,
            })
        })
        .collect()
}

pub async fn search_models(
    query: &str,
    sort_by: &str,
    limit: u32,
    token: Option<&str>,
) -> Result<Vec<HfModelCard>, HfError> {
    let url = search_url(query, sort_by, limit);
    let body = get_text(url, token).await?;
    Ok(parse_search_response(&body))
}

// ── Repo file listing ───────────────────────────────────────────────────────

pub fn repo_info_url(repo_id: &str) -> String {
    format!("https://huggingface.co/api/models/{repo_id}")
}

pub fn parse_all_repo_files(json: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    let Some(siblings) = v["siblings"].as_array() else {
        return vec![];
    };
    siblings
        .iter()
        .filter_map(|s| s["rfilename"].as_str().map(str::to_owned))
        .collect()
}

pub fn parse_repo_model_files(json: &str) -> Vec<String> {
    parse_all_repo_files(json)
        .into_iter()
        .filter(|f| MODEL_EXTENSIONS.iter().any(|ext| f.ends_with(ext)))
        .collect()
}

/// Downloadable model filenames (`.pth` and `.zip`) from a HuggingFace repo.
pub async fn list_repo_model_files(
    repo_id: &str,
    token: Option<&str>,
) -> Result<Vec<String>, HfError> {
    let body = get_text(repo_info_url(repo_id), token).await?;
    Ok(parse_repo_model_files(&body))
}

async fn list_all_repo_files(repo_id: &str, token: Option<&str>) -> Result<Vec<String>, HfError> {
    let body = get_text(repo_info_url(repo_id), token).await?;
    Ok(parse_all_repo_files(&body))
}

/// Pick the `.index` file matching `stem`, or the sole `.index` file in the
/// repo when there is no ambiguity. Mirrors the Python reference's tie-break.
pub fn pick_index_file(all_index_files: &[String], stem: &str) -> Option<String> {
    if let Some(f) = all_index_files.iter().find(|f| {
        Path::new(f.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.contains(stem))
    }) {
        return Some(f.clone());
    }
    if all_index_files.len() == 1 {
        return Some(all_index_files[0].clone());
    }
    None
}

// ── Zip archive extraction ──────────────────────────────────────────────────
// Direct port of `hf_search.py`'s `_extract_pth_from_zip`.

/// True when `name` is a real, extractable member for `ext`: has the
/// extension, isn't a hidden/dotfile, and isn't inside macOS's `__MACOSX`
/// resource-fork junk directory.
fn is_relevant_zip_member(name: &str, ext: &str) -> bool {
    if !name.ends_with(ext) {
        return false;
    }
    if name.contains("__MACOSX") {
        return false;
    }
    let base = Path::new(name)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    !base.starts_with('.')
}

fn zip_members_with_ext(names: &[String], ext: &str) -> Vec<String> {
    names
        .iter()
        .filter(|n| is_relevant_zip_member(n, ext))
        .cloned()
        .collect()
}

/// Pair one `.index` archive member with the extracted model stem it belongs
/// to: prefer a stem that is a substring of the index member's own stem,
/// else fall back to the sole extracted model when there is no ambiguity.
fn match_index_member_to_stem(index_member: &str, extracted_stems: &[String]) -> Option<String> {
    let mname = Path::new(index_member)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if let Some(s) = extracted_stems.iter().find(|s| mname.contains(s.as_str())) {
        return Some(s.clone());
    }
    if extracted_stems.len() == 1 {
        return Some(extracted_stems[0].clone());
    }
    None
}

/// Extract every `.pth` file (and matching `.index` sidecars) from a zip
/// archive's raw bytes into `dest_folder`. Returns the extracted models'
/// stem names. `.pth` files keep their original archive filename; `.index`
/// files are renamed to `<matched stem>.index`.
fn extract_pth_from_zip(zip_bytes: &[u8], dest_folder: &Path) -> Result<Vec<String>, HfError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|e| HfError::Zip(e.to_string()))?;

    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| HfError::Zip(e.to_string()))?;
        names.push(entry.name().to_owned());
    }

    let pth_members = zip_members_with_ext(&names, ".pth");
    if pth_members.is_empty() {
        return Ok(vec![]);
    }

    std::fs::create_dir_all(dest_folder).map_err(|e| HfError::Io(e.to_string()))?;

    let mut extracted = Vec::with_capacity(pth_members.len());
    for member in &pth_members {
        let file_name = Path::new(member)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(member.as_str());
        let target = dest_folder.join(file_name);
        let buf = read_zip_member(&mut archive, member)?;
        std::fs::write(&target, &buf).map_err(|e| HfError::Io(e.to_string()))?;
        extracted.push(
            target
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned(),
        );
    }

    for member in zip_members_with_ext(&names, ".index") {
        let Some(stem) = match_index_member_to_stem(&member, &extracted) else {
            continue;
        };
        let buf = read_zip_member(&mut archive, &member)?;
        std::fs::write(dest_folder.join(format!("{stem}.index")), &buf)
            .map_err(|e| HfError::Io(e.to_string()))?;
    }

    Ok(extracted)
}

fn read_zip_member(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    member: &str,
) -> Result<Vec<u8>, HfError> {
    let mut file = archive
        .by_name(member)
        .map_err(|e| HfError::Zip(e.to_string()))?;
    let mut buf = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut buf)
        .map_err(|e| HfError::Io(e.to_string()))?;
    Ok(buf)
}

// ── Download ─────────────────────────────────────────────────────────────────

pub fn resolve_url(repo_id: &str, filename: &str) -> String {
    format!("https://huggingface.co/{repo_id}/resolve/main/{filename}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadOutcome {
    /// Stem(s) of the downloaded/extracted `.pth` file(s) — one for a bare
    /// `.pth` download, possibly several for a `.zip` bundle.
    pub model_names: Vec<String>,
    pub index_downloaded: bool,
}

/// Download a model from HuggingFace into `dest_folder`. A bare `.pth` is
/// saved directly, then its matching FAISS `.index` sidecar is best-effort
/// fetched from the repo (renamed to `<model stem>.index`). A `.zip` archive
/// is extracted in place — `.pth` files verbatim, `.index` files paired and
/// renamed by `extract_pth_from_zip` — with no further repo lookup, since
/// RVC WebUI bundles ship the index alongside the model in the same archive.
pub async fn download_model(
    repo_id: &str,
    filename: &str,
    dest_folder: &Path,
    token: Option<&str>,
) -> Result<DownloadOutcome, HfError> {
    let bytes = get_bytes(resolve_url(repo_id, filename), token).await?;

    if filename.ends_with(".zip") {
        let model_names = extract_pth_from_zip(&bytes, dest_folder)?;
        if model_names.is_empty() {
            return Err(HfError::Zip("no .pth files found inside zip".to_owned()));
        }
        let index_downloaded = model_names
            .iter()
            .any(|stem| dest_folder.join(format!("{stem}.index")).is_file());
        return Ok(DownloadOutcome {
            model_names,
            index_downloaded,
        });
    }

    std::fs::create_dir_all(dest_folder).map_err(|e| HfError::Io(e.to_string()))?;
    let dest_name = Path::new(filename)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(filename);
    let dest_path = dest_folder.join(dest_name);
    std::fs::write(&dest_path, &bytes).map_err(|e| HfError::Io(e.to_string()))?;

    let stem = dest_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();

    let index_downloaded = download_matching_index(repo_id, &stem, dest_folder, token).await;

    Ok(DownloadOutcome {
        model_names: vec![stem],
        index_downloaded,
    })
}

async fn download_matching_index(
    repo_id: &str,
    stem: &str,
    dest_folder: &Path,
    token: Option<&str>,
) -> bool {
    let Ok(all_files) = list_all_repo_files(repo_id, token).await else {
        return false;
    };
    let index_files: Vec<String> = all_files
        .into_iter()
        .filter(|f| f.ends_with(".index"))
        .collect();
    let Some(chosen) = pick_index_file(&index_files, stem) else {
        return false;
    };
    let Ok(bytes) = get_bytes(resolve_url(repo_id, &chosen), token).await else {
        return false;
    };
    std::fs::write(dest_folder.join(format!("{stem}.index")), &bytes).is_ok()
}

pub fn delete_model_files(dest_folder: &Path, stem: &str) -> bool {
    let path = dest_folder.join(format!("{stem}.pth"));
    path.is_file() && std::fs::remove_file(&path).is_ok()
}

// ── HTTP plumbing ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum HfError {
    Http(String),
    Io(String),
    Zip(String),
}

impl std::fmt::Display for HfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HfError::Http(msg) => write!(f, "HuggingFace request failed: {msg}"),
            HfError::Io(msg) => write!(f, "filesystem error: {msg}"),
            HfError::Zip(msg) => write!(f, "zip archive error: {msg}"),
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("linux-arctis-manager/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client builder should not fail with default settings")
}

async fn get_text<U: reqwest::IntoUrl>(url: U, token: Option<&str>) -> Result<String, HfError> {
    let mut req = client().get(url);
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| HfError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(HfError::Http(format!("HTTP {}", resp.status())));
    }
    resp.text().await.map_err(|e| HfError::Http(e.to_string()))
}

async fn get_bytes<U: reqwest::IntoUrl>(url: U, token: Option<&str>) -> Result<Vec<u8>, HfError> {
    let mut req = client().get(url);
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| HfError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(HfError::Http(format!("HTTP {}", resp.status())));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| HfError::Http(e.to_string()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── Token management ────────────────────────────────────────────────

    #[test]
    fn set_and_get_token_roundtrip() {
        let base = tempdir().unwrap();
        set_token(base.path(), "  hf_abc123  ").unwrap();
        // The daemon's own token file always wins, regardless of the real
        // environment's HF_TOKEN or huggingface-cli login cache.
        assert_eq!(get_token(base.path()).as_deref(), Some("hf_abc123"));
    }

    // ── pick_token priority (pure — no environment/filesystem dependency) ──

    #[test]
    fn pick_token_none_when_nothing_set() {
        assert_eq!(pick_token(None, None, None), None);
    }

    #[test]
    fn pick_token_own_file_wins_over_env_and_cache() {
        assert_eq!(
            pick_token(
                Some("own".to_owned()),
                Some("env".to_owned()),
                Some("cache".to_owned())
            ),
            Some("own".to_owned())
        );
    }

    #[test]
    fn pick_token_env_wins_over_cache_when_own_absent() {
        assert_eq!(
            pick_token(None, Some("env".to_owned()), Some("cache".to_owned())),
            Some("env".to_owned())
        );
    }

    #[test]
    fn pick_token_falls_back_to_cache() {
        assert_eq!(
            pick_token(None, None, Some("cache".to_owned())),
            Some("cache".to_owned())
        );
    }

    // ── Search URL / parsing ────────────────────────────────────────────

    #[test]
    fn search_url_blank_query_browses_rvc_tag() {
        let url = search_url("", "downloads", 20);
        assert!(url.query().unwrap().contains("filter=rvc"));
        assert!(!url.query().unwrap().contains("search="));
    }

    #[test]
    fn search_url_with_query_uses_search_param_not_filter() {
        let url = search_url("my voice", "likes", 10);
        assert!(url.query().unwrap().contains("search=my+voice"));
        assert!(!url.query().unwrap().contains("filter=rvc"));
    }

    #[test]
    fn search_url_invalid_sort_falls_back_to_downloads() {
        let url = search_url("", "not_a_real_sort", 20);
        assert!(url.query().unwrap().contains("sort=downloads"));
    }

    #[test]
    fn parse_search_response_extracts_fields() {
        let json = r#"[{"id": "someuser/my-rvc-model", "author": "someuser", "downloads": 42, "likes": 7}]"#;
        let models = parse_search_response(json);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].repo_id, "someuser/my-rvc-model");
        assert_eq!(models[0].name, "my-rvc-model");
        assert_eq!(models[0].downloads, 42);
        assert_eq!(models[0].likes, 7);
    }

    #[test]
    fn parse_search_response_returns_empty_on_bad_json() {
        assert!(parse_search_response("not json").is_empty());
    }

    // ── Repo file listing ────────────────────────────────────────────────

    #[test]
    fn parse_repo_model_files_filters_to_pth_and_zip() {
        let json = r#"{"siblings": [
            {"rfilename": "model.pth"},
            {"rfilename": "README.md"},
            {"rfilename": "bundle.zip"},
            {"rfilename": "model.index"}
        ]}"#;
        let files = parse_repo_model_files(json);
        assert_eq!(files, vec!["model.pth".to_owned(), "bundle.zip".to_owned()]);
    }

    #[test]
    fn parse_all_repo_files_returns_everything() {
        let json = r#"{"siblings": [{"rfilename": "a.pth"}, {"rfilename": "b.index"}]}"#;
        assert_eq!(
            parse_all_repo_files(json),
            vec!["a.pth".to_owned(), "b.index".to_owned()]
        );
    }

    // ── Index tie-break (pick_index_file) ───────────────────────────────

    #[test]
    fn pick_index_file_prefers_stem_match() {
        let files = vec![
            "added_IVF256_other_voice_v2.index".to_owned(),
            "added_IVF256_my_voice_v2.index".to_owned(),
        ];
        assert_eq!(
            pick_index_file(&files, "my_voice"),
            Some("added_IVF256_my_voice_v2.index".to_owned())
        );
    }

    #[test]
    fn pick_index_file_falls_back_to_sole_unambiguous_file() {
        let files = vec!["added_IVF256_v2.index".to_owned()];
        assert_eq!(
            pick_index_file(&files, "my_voice"),
            Some("added_IVF256_v2.index".to_owned())
        );
    }

    #[test]
    fn pick_index_file_none_when_ambiguous_and_no_stem_match() {
        let files = vec!["a.index".to_owned(), "b.index".to_owned()];
        assert_eq!(pick_index_file(&files, "my_voice"), None);
    }

    #[test]
    fn pick_index_file_none_when_empty() {
        assert_eq!(pick_index_file(&[], "my_voice"), None);
    }

    // ── URL building ─────────────────────────────────────────────────────

    #[test]
    fn resolve_url_builds_expected_download_link() {
        assert_eq!(
            resolve_url("someuser/my-model", "model.pth"),
            "https://huggingface.co/someuser/my-model/resolve/main/model.pth"
        );
    }

    #[test]
    fn repo_info_url_builds_expected_api_link() {
        assert_eq!(
            repo_info_url("someuser/my-model"),
            "https://huggingface.co/api/models/someuser/my-model"
        );
    }

    // ── Delete ───────────────────────────────────────────────────────────

    #[test]
    fn delete_model_files_removes_pth() {
        let dir = tempdir().unwrap();
        let pth = dir.path().join("voice_a.pth");
        std::fs::write(&pth, b"").unwrap();
        assert!(delete_model_files(dir.path(), "voice_a"));
        assert!(!pth.exists());
    }

    #[test]
    fn delete_model_files_false_when_absent() {
        let dir = tempdir().unwrap();
        assert!(!delete_model_files(dir.path(), "missing"));
    }

    // ── Zip extraction ───────────────────────────────────────────────────

    fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn extract_pth_from_zip_extracts_single_model_and_index() {
        let zip_bytes = build_test_zip(&[
            ("voice_a.pth", b"pth-bytes"),
            ("added_IVF256_voice_a_v2.index", b"index-bytes"),
            ("README.md", b"not a model"),
        ]);
        let dir = tempdir().unwrap();

        let names = extract_pth_from_zip(&zip_bytes, dir.path()).unwrap();

        assert_eq!(names, vec!["voice_a".to_owned()]);
        assert_eq!(
            std::fs::read(dir.path().join("voice_a.pth")).unwrap(),
            b"pth-bytes"
        );
        assert_eq!(
            std::fs::read(dir.path().join("voice_a.index")).unwrap(),
            b"index-bytes"
        );
        assert!(!dir.path().join("README.md").exists());
    }

    #[test]
    fn extract_pth_from_zip_skips_macosx_and_dotfiles() {
        let zip_bytes = build_test_zip(&[
            ("voice_a.pth", b"real"),
            ("__MACOSX/._voice_a.pth", b"junk"),
            (".hidden.pth", b"junk"),
        ]);
        let dir = tempdir().unwrap();

        let names = extract_pth_from_zip(&zip_bytes, dir.path()).unwrap();

        assert_eq!(names, vec!["voice_a".to_owned()]);
        assert_eq!(
            std::fs::read(dir.path().join("voice_a.pth")).unwrap(),
            b"real"
        );
    }

    #[test]
    fn extract_pth_from_zip_empty_when_no_pth_members() {
        let zip_bytes = build_test_zip(&[("README.md", b"nothing here")]);
        let dir = tempdir().unwrap();
        assert!(extract_pth_from_zip(&zip_bytes, dir.path())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn extract_pth_from_zip_multi_model_pairs_index_by_stem() {
        let zip_bytes = build_test_zip(&[
            ("voice_a.pth", b"a"),
            ("voice_b.pth", b"b"),
            ("added_IVF256_voice_b_v2.index", b"idx-b"),
        ]);
        let dir = tempdir().unwrap();

        let mut names = extract_pth_from_zip(&zip_bytes, dir.path()).unwrap();
        names.sort();

        assert_eq!(names, vec!["voice_a".to_owned(), "voice_b".to_owned()]);
        assert!(!dir.path().join("voice_a.index").exists());
        assert_eq!(
            std::fs::read(dir.path().join("voice_b.index")).unwrap(),
            b"idx-b"
        );
    }

    #[test]
    fn match_index_member_to_stem_prefers_substring_match() {
        let stems = vec!["voice_a".to_owned(), "voice_b".to_owned()];
        assert_eq!(
            match_index_member_to_stem("added_IVF256_voice_b_v2.index", &stems),
            Some("voice_b".to_owned())
        );
    }

    #[test]
    fn match_index_member_to_stem_falls_back_when_single_model() {
        let stems = vec!["voice_a".to_owned()];
        assert_eq!(
            match_index_member_to_stem("added_IVF256_v2.index", &stems),
            Some("voice_a".to_owned())
        );
    }

    #[test]
    fn match_index_member_to_stem_none_when_ambiguous() {
        let stems = vec!["voice_a".to_owned(), "voice_b".to_owned()];
        assert_eq!(
            match_index_member_to_stem("added_IVF256_v2.index", &stems),
            None
        );
    }

    #[test]
    fn is_relevant_zip_member_filters_extension_hidden_and_macosx() {
        assert!(is_relevant_zip_member("voice_a.pth", ".pth"));
        assert!(!is_relevant_zip_member("voice_a.index", ".pth"));
        assert!(!is_relevant_zip_member(".hidden.pth", ".pth"));
        assert!(!is_relevant_zip_member("__MACOSX/voice_a.pth", ".pth"));
    }
}
