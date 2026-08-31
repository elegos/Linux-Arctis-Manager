// Brute-force weighted k-NN retrieval over a model's FAISS `.index` feature
// vectors ([E10-S6a]) — replaces the `libfaiss` C++ dependency the Python
// daemon links, per the design decided in `docs/voice-changing-feature.md`
// (Phase 3): community `.index` files are small enough (a few hundred
// thousand low-hundreds-dimension vectors) that brute force is cheap and
// avoids the C++ dependency entirely.
//
// `.index` files are written by `faiss.write_index` on a plain
// `IndexIVFFlat` (RVC WebUI never uses PQ/scalar quantization for these).
// The on-disk format was reverse-engineered from `facebookresearch/faiss`'s
// real `index_read.cpp`/`io_macros.h` source on GitHub (not reconstructed
// from memory) and cross-checked byte-exact against four real downloaded
// RVC model `.index` files (51k-164k vectors each, 160-520 MB) — every
// parse consumed the file to the last byte and the inverted lists' vector
// counts summed to exactly `ntotal`. See `docs/v3-backlog.md` [E10-S6a] for
// that verification session's notes.
//
// Layout (all fields little-endian, matching native x86_64 `fwrite`):
//
//   fourcc "IwFl"                          top-level: IndexIVFFlat
//   index_header                            (see `read_index_header`)
//   u64 nlist, u64 nprobe
//   nested index (the coarse quantizer)     skipped — not needed for brute force
//   direct_map                              skipped (RVC WebUI never enables it)
//   fourcc "ilar"                           inverted lists: plain array format
//   u64 nlist (must match), u64 code_size (must match d*4)
//   fourcc "full", u64 nlist, u64[nlist] per-list vector counts
//   per non-empty list: <count>*code_size code bytes, then <count>*8 id bytes
//
// We only support "IwFl" + "ilar" + "full" — the only combination RVC
// WebUI's `train_index` script ever produces. Anything else (PQ-compressed
// indexes, sparse list encoding, a maintained direct map) is rejected with
// a clear error rather than silently mishandled.

use std::path::Path;

#[derive(Debug)]
pub enum RetrievalError {
    Io(String),
    Format(String),
}

impl std::fmt::Display for RetrievalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetrievalError::Io(msg) => write!(f, "I/O error reading .index: {msg}"),
            RetrievalError::Format(msg) => write!(f, "unsupported or malformed .index: {msg}"),
        }
    }
}

/// A loaded `.index` feature database: every training-set vector, indexed by
/// its original FAISS id (`0..n_vectors`, contiguous — RVC WebUI always adds
/// vectors via `add_with_ids(vecs, arange(len(vecs)))`).
#[derive(Debug)]
pub struct RetrievalIndex {
    pub dim: usize,
    pub n_vectors: usize,
    feats: Vec<f32>, // flat [n_vectors, dim], row i = the vector added with FAISS id i
}

impl RetrievalIndex {
    pub fn vector(&self, id: usize) -> &[f32] {
        &self.feats[id * self.dim..(id + 1) * self.dim]
    }

    pub fn load(path: &Path) -> Result<Self, RetrievalError> {
        let data = std::fs::read(path).map_err(|e| RetrievalError::Io(e.to_string()))?;
        parse_ivfflat(&data)
    }
}

// ── Byte-level reader ────────────────────────────────────────────────────

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], RetrievalError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| {
                RetrievalError::Format(format!("unexpected end of file at offset {}", self.pos))
            })?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, RetrievalError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, RetrievalError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, RetrievalError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, RetrievalError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, RetrievalError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn fourcc(&mut self) -> Result<[u8; 4], RetrievalError> {
        Ok(self.take(4)?.try_into().unwrap())
    }

    /// Read `n` little-endian f32 values (`n*4` bytes) into a fresh `Vec`.
    fn f32_vec(&mut self, n: usize) -> Result<Vec<f32>, RetrievalError> {
        let bytes = self.take(n * 4)?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect())
    }
}

struct IndexHeader {
    d: i32,
    ntotal: i64,
}

/// `faiss::read_index_header` — common to every `Index` subclass.
fn read_index_header(r: &mut Reader) -> Result<IndexHeader, RetrievalError> {
    let d = r.i32()?;
    let ntotal = r.i64()?;
    r.i64()?; // dummy (legacy field)
    r.i64()?; // dummy (legacy field)
    r.u8()?; // is_trained
    let metric_type = r.i32()?;
    if metric_type > 1 {
        r.f32()?; // metric_arg
    }
    Ok(IndexHeader { d, ntotal })
}

/// Skip over the coarse quantizer (a nested `IndexFlat`) — its centroids
/// aren't needed for brute-force search over the full vector set, but the
/// bytes must still be consumed to reach the inverted lists that follow.
fn skip_nested_flat_index(r: &mut Reader) -> Result<(), RetrievalError> {
    let h = r.fourcc()?;
    if !matches!(&h, b"IxFI" | b"IxF2" | b"IxFl") {
        return Err(RetrievalError::Format(format!(
            "unsupported coarse quantizer type {:?} (only plain IndexFlat is supported)",
            String::from_utf8_lossy(&h)
        )));
    }
    let hdr = read_index_header(r)?;
    let n_floats = r.u64()?; // `codes` is a byte vector on the C++ side: the
                             // stored size_t IS the float count directly (code_size/4 bytes-per-float
                             // cancel out), not float_count/4 as the WRITEXBVECTOR macro's naming
                             // suggests at a glance — confirmed against four real files.
    let expected = hdr.ntotal.max(0) as u64 * hdr.d.max(0) as u64;
    if n_floats != expected {
        return Err(RetrievalError::Format(format!(
            "coarse quantizer vector count mismatch: {n_floats} != {expected}"
        )));
    }
    r.take((n_floats as usize) * 4)?; // codes, discarded
    Ok(())
}

/// Skip `faiss::read_direct_map` — RVC WebUI never enables `maintain_direct_map`.
fn skip_direct_map(r: &mut Reader) -> Result<(), RetrievalError> {
    let maintain = r.u8()?;
    let array_len = r.u64()?;
    r.take((array_len as usize) * 8)?;
    if maintain == 2 {
        // DirectMap::Hashtable: vector<pair<idx_t, idx_t>>, 16 bytes/entry
        let n = r.u64()?;
        r.take((n as usize) * 16)?;
    }
    Ok(())
}

fn parse_ivfflat(data: &[u8]) -> Result<RetrievalIndex, RetrievalError> {
    let mut r = Reader::new(data);

    let top = r.fourcc()?;
    if &top != b"IwFl" {
        return Err(RetrievalError::Format(format!(
            "unsupported index type {:?} (only plain IndexIVFFlat \"IwFl\" is supported)",
            String::from_utf8_lossy(&top)
        )));
    }

    let hdr = read_index_header(&mut r)?;
    if hdr.d <= 0 || hdr.ntotal < 0 {
        return Err(RetrievalError::Format(format!(
            "invalid header: d={} ntotal={}",
            hdr.d, hdr.ntotal
        )));
    }
    let dim = hdr.d as usize;
    let n_vectors = hdr.ntotal as usize;

    let _nlist = r.u64()?;
    let _nprobe = r.u64()?;
    skip_nested_flat_index(&mut r)?;
    skip_direct_map(&mut r)?;

    let il_kind = r.fourcc()?;
    if &il_kind != b"ilar" {
        return Err(RetrievalError::Format(format!(
            "unsupported inverted list type {:?} (only \"ilar\" is supported)",
            String::from_utf8_lossy(&il_kind)
        )));
    }
    let il_nlist = r.u64()? as usize;
    let code_size = r.u64()? as usize;
    let expected_code_size = dim * 4;
    if code_size != expected_code_size {
        return Err(RetrievalError::Format(format!(
            "code_size {code_size} != d*4 ({expected_code_size})"
        )));
    }

    let list_type = r.fourcc()?;
    let sizes: Vec<usize> = if &list_type == b"full" {
        let n = r.u64()? as usize;
        if n != il_nlist {
            return Err(RetrievalError::Format(format!(
                "'full' sizes count {n} != nlist {il_nlist}"
            )));
        }
        (0..n)
            .map(|_| r.u64().map(|v| v as usize))
            .collect::<Result<_, _>>()?
    } else {
        return Err(RetrievalError::Format(format!(
            "unsupported inverted list size encoding {:?} (only \"full\" is supported)",
            String::from_utf8_lossy(&list_type)
        )));
    };

    let mut feats = vec![0.0f32; n_vectors * dim];
    let mut written = vec![false; n_vectors];
    for &n in &sizes {
        if n == 0 {
            continue;
        }
        let codes = r.f32_vec(n * dim)?;
        let ids_bytes = r.take(n * 8)?;
        for j in 0..n {
            let id = i64::from_le_bytes(ids_bytes[j * 8..(j + 1) * 8].try_into().unwrap());
            if id < 0 || id as usize >= n_vectors {
                return Err(RetrievalError::Format(format!(
                    "vector id {id} out of range [0, {n_vectors})"
                )));
            }
            let id = id as usize;
            feats[id * dim..(id + 1) * dim].copy_from_slice(&codes[j * dim..(j + 1) * dim]);
            written[id] = true;
        }
    }
    if let Some(missing) = written.iter().position(|&w| !w) {
        return Err(RetrievalError::Format(format!(
            "vector id {missing} never appeared in any inverted list"
        )));
    }

    Ok(RetrievalIndex {
        dim,
        n_vectors,
        feats,
    })
}

// ── Brute-force weighted k-NN ────────────────────────────────────────────

fn squared_l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x - y) * (x - y))
        .sum()
}

/// The `k` nearest neighbours of `query` in `index`, by squared L2 distance
/// (ascending) — matches `faiss::IndexIVFFlat::search`'s `METRIC_L2` scoring
/// (squared, not the plain norm). Returns `(id, squared_distance)` pairs.
pub fn knn_search(index: &RetrievalIndex, query: &[f32], k: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = (0..index.n_vectors)
        .map(|id| (id, squared_l2(index.vector(id), query)))
        .collect();
    scored.sort_by(|a, b| a.1.total_cmp(&b.1));
    scored.truncate(k);
    scored
}

/// FAISS feature-retrieval blend (RVC WebUI's "index rate"): replace each
/// frame of `feats` (`[n_frames, dim]`, row-major, mutated in place) with a
/// distance-weighted mix of its `k` nearest training-set vectors in `index`.
/// A no-op when `index_rate <= 0`. Port of the retrieval block in
/// `pipeline.py::_run_inference`.
pub fn retrieval_blend(
    index: &RetrievalIndex,
    feats: &mut [f32],
    n_frames: usize,
    dim: usize,
    k: usize,
    index_rate: f32,
) {
    if index_rate <= 0.0 {
        return;
    }
    assert_eq!(feats.len(), n_frames * dim);
    assert_eq!(index.dim, dim);

    for t in 0..n_frames {
        let frame = feats[t * dim..(t + 1) * dim].to_vec();
        let neighbors = knn_search(index, &frame, k);

        let mut weights: Vec<f32> = neighbors
            .iter()
            .map(|&(_, score)| (1.0 / score.max(1e-6)).powi(2))
            .collect();
        let weight_sum: f32 = weights.iter().sum();
        for w in weights.iter_mut() {
            *w /= weight_sum;
        }

        let mut retrieved = vec![0.0f32; dim];
        for (&(id, _), &w) in neighbors.iter().zip(weights.iter()) {
            let v = index.vector(id);
            for i in 0..dim {
                retrieved[i] += v[i] * w;
            }
        }

        let out = &mut feats[t * dim..(t + 1) * dim];
        for i in 0..dim {
            out[i] = index_rate * retrieved[i] + (1.0 - index_rate) * out[i];
        }
    }
}

#[cfg(test)]
#[allow(clippy::excessive_precision)] // fixture values pasted verbatim from the Python reference
mod tests {
    use super::*;

    // ── file format parser — a hand-built minimal fixture ───────────────

    struct FixtureWriter {
        buf: Vec<u8>,
    }
    impl FixtureWriter {
        fn new() -> Self {
            FixtureWriter { buf: Vec::new() }
        }
        fn fourcc(&mut self, s: &[u8; 4]) -> &mut Self {
            self.buf.extend_from_slice(s);
            self
        }
        fn i32(&mut self, v: i32) -> &mut Self {
            self.buf.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn i64(&mut self, v: i64) -> &mut Self {
            self.buf.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u64(&mut self, v: u64) -> &mut Self {
            self.buf.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn u8(&mut self, v: u8) -> &mut Self {
            self.buf.push(v);
            self
        }
        fn f32(&mut self, v: f32) -> &mut Self {
            self.buf.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn index_header(&mut self, d: i32, ntotal: i64) -> &mut Self {
            self.i32(d).i64(ntotal).i64(0).i64(0).u8(1); // is_trained=true
            self.i32(1) // metric_type = METRIC_L2 (no metric_arg follows)
        }
    }

    /// A tiny but format-faithful `IndexIVFFlat`: d=2, 3 vectors split
    /// across 2 inverted lists (sizes [2, 1]), NoMap direct map, a minimal
    /// (1-vector) coarse quantizer.
    fn build_fixture() -> Vec<u8> {
        let mut w = FixtureWriter::new();
        w.fourcc(b"IwFl");
        w.index_header(2, 3); // d=2, ntotal=3

        w.u64(2); // nlist
        w.u64(1); // nprobe

        // Nested coarse quantizer: IndexFlatL2 with 1 centroid, d=2.
        w.fourcc(b"IxF2");
        w.index_header(2, 1);
        w.u64(2); // float count (1 vector * 2 dims)
        w.f32(0.0).f32(0.0);

        // direct_map: NoMap, empty array.
        w.u8(0).u64(0);

        // Inverted lists: "ilar", nlist=2, code_size=8 (2 floats * 4 bytes).
        w.fourcc(b"ilar");
        w.u64(2);
        w.u64(8);
        w.fourcc(b"full");
        w.u64(2); // sizes count == nlist
        w.u64(2); // list 0 has 2 vectors
        w.u64(1); // list 1 has 1 vector

        // list 0: ids {0, 2}, vectors (1.0, 2.0) and (5.0, 6.0)
        w.f32(1.0).f32(2.0);
        w.f32(5.0).f32(6.0);
        w.i64(0);
        w.i64(2);

        // list 1: id {1}, vector (3.0, 4.0)
        w.f32(3.0).f32(4.0);
        w.i64(1);

        w.buf
    }

    #[test]
    fn parses_the_hand_built_fixture_correctly() {
        let bytes = build_fixture();
        let idx = parse_ivfflat(&bytes).expect("fixture should parse");
        assert_eq!(idx.dim, 2);
        assert_eq!(idx.n_vectors, 3);
        assert_eq!(idx.vector(0), &[1.0, 2.0]);
        assert_eq!(idx.vector(1), &[3.0, 4.0]);
        assert_eq!(idx.vector(2), &[5.0, 6.0]);
    }

    #[test]
    fn rejects_unsupported_top_level_type() {
        let mut w = FixtureWriter::new();
        w.fourcc(b"IxPQ");
        let err = parse_ivfflat(&w.buf).unwrap_err();
        assert!(matches!(err, RetrievalError::Format(_)));
    }

    #[test]
    fn rejects_truncated_file() {
        let bytes = build_fixture();
        let truncated = &bytes[..bytes.len() - 10];
        assert!(parse_ivfflat(truncated).is_err());
    }

    #[test]
    fn rejects_out_of_range_id() {
        // Same as build_fixture but list 1's id is 99 instead of 1.
        let mut w = FixtureWriter::new();
        w.fourcc(b"IwFl");
        w.index_header(2, 3);
        w.u64(2).u64(1);
        w.fourcc(b"IxF2");
        w.index_header(2, 1);
        w.u64(2);
        w.f32(0.0).f32(0.0);
        w.u8(0).u64(0);
        w.fourcc(b"ilar");
        w.u64(2).u64(8);
        w.fourcc(b"full");
        w.u64(2).u64(2).u64(1);
        w.f32(1.0).f32(2.0);
        w.f32(5.0).f32(6.0);
        w.i64(0).i64(2);
        w.f32(3.0).f32(4.0);
        w.i64(99);
        let err = parse_ivfflat(&w.buf).unwrap_err();
        assert!(matches!(err, RetrievalError::Format(_)));
    }

    // ── knn_search / retrieval_blend — reference values from gen_retrieval_vectors.py

    fn make_index(dim: usize, flat: &[f32]) -> RetrievalIndex {
        let n_vectors = flat.len() / dim;
        RetrievalIndex {
            dim,
            n_vectors,
            feats: flat.to_vec(),
        }
    }

    fn db_fixture() -> RetrievalIndex {
        #[rustfmt::skip]
        let db: [f32; 48] = [
            1.74945474, -0.28607300, -0.48456514, -2.65331864,
            -0.00828463, -0.31963137, -0.53662938, 0.31540266,
            0.42105073, -1.06560302, -0.88623965, -0.47573349,
            0.68968230, 0.56119215, -1.30554855, -1.11947525,
            0.73683739, 1.57463408, -0.03107509, -0.68344665,
            1.09562969, -0.30957663, 0.72575223, 1.54907167,
            0.63007981, 0.07349323, 0.73227137, -0.64257538,
            -0.17809318, -0.57395458, -0.20437531, -0.48649511,
            -0.18577532, -0.38053641, 0.08897763, 0.06367166,
            0.29634711, 1.40277112, -1.54686260, 1.29561853,
            -0.23725045, -1.23234618, -0.17241977, 0.09183837,
            1.06755841, -1.06163442, 0.21734820, 0.11781950,
        ];
        make_index(4, &db)
    }

    #[test]
    fn knn_search_matches_python_reference() {
        let index = db_fixture();
        let query = [-1.68411088f32, -1.18575525, 0.60010201, 0.69556725]; // frame 0
        let neighbors = knn_search(&index, &query, 4);
        let expected_ids = [10usize, 8, 7, 1];
        let expected_scores = [
            3.056854248046875f32,
            3.553927183151245,
            4.686844825744629,
            4.9952473640441895,
        ];
        for (i, &(id, score)) in neighbors.iter().enumerate() {
            assert_eq!(id, expected_ids[i], "neighbor {i}");
            assert!(
                (score - expected_scores[i]).abs() < 1e-3,
                "score {i}: {score} vs {}",
                expected_scores[i]
            );
        }
    }

    #[test]
    fn knn_search_self_match_has_zero_distance() {
        let index = db_fixture();
        let self_query = index.vector(5).to_vec();
        let neighbors = knn_search(&index, &self_query, 1);
        assert_eq!(neighbors[0], (5, 0.0));
    }

    #[test]
    fn retrieval_blend_matches_python_reference() {
        let index = db_fixture();
        #[rustfmt::skip]
        let mut query: [f32; 12] = [
            -1.68411088, -1.18575525, 0.60010201, 0.69556725,
            1.08771086, 0.53382170, 0.39521202, 0.12286753,
            1.20910168, -0.84306610, -0.14189358, 0.38535413,
        ];
        retrieval_blend(&index, &mut query, 3, 4, 4, 0.6);
        let expected = [
            -0.78079557f32,
            -0.91791177,
            0.14684324,
            0.29006478,
            0.79817128,
            0.31175423,
            0.43639964,
            -0.24114549,
            1.09565878,
            -0.96072799,
            0.05249401,
            0.22031683,
        ];
        for (got, want) in query.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }

    #[test]
    fn retrieval_blend_is_noop_when_index_rate_is_zero() {
        let index = db_fixture();
        let mut query = [1.0f32, 2.0, 3.0, 4.0];
        let before = query;
        retrieval_blend(&index, &mut query, 1, 4, 4, 0.0);
        assert_eq!(query, before);
    }

    /// Not run by default (`cargo test`) — needs a real downloaded RVC
    /// model's `.index` file on disk. Run manually with
    /// `cargo test --bin lam-daemon -- --ignored live_loads_a_real_index_file`
    /// after downloading a model with a FAISS index, to sanity-check the
    /// parser end to end against a real multi-hundred-MB file (this session
    /// verified byte-exact parsing against four real `.index` files ranging
    /// 51k-164k vectors / 160-520 MB — see docs/v3-backlog.md [E10-S6a]).
    #[test]
    #[ignore]
    fn live_loads_a_real_index_file() {
        let path = std::env::var("LAM_TEST_INDEX_PATH").unwrap_or_else(|_| {
            format!(
                "{}/.config/arctis_manager/rvc_models/rosaria-jp.index",
                std::env::var("HOME").expect("HOME not set")
            )
        });
        let index = RetrievalIndex::load(std::path::Path::new(&path))
            .unwrap_or_else(|e| panic!("failed to load {path}: {e}"));
        assert!(index.dim > 0);
        assert!(index.n_vectors > 0);
        eprintln!(
            "loaded {path}: dim={} n_vectors={}",
            index.dim, index.n_vectors
        );

        // Self-consistency: any vector's nearest neighbor to itself must be
        // itself, at distance 0 — doesn't need a precomputed reference.
        let probe_id = index.n_vectors / 2;
        let probe = index.vector(probe_id).to_vec();
        let neighbors = knn_search(&index, &probe, 1);
        assert_eq!(neighbors[0], (probe_id, 0.0));
    }
}
