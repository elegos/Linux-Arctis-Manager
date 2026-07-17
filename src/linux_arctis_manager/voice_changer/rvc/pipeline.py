from __future__ import annotations

import logging
import math
import threading
import wave
from collections import deque
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F

logger = logging.getLogger('RVCPipeline')

# ── Debug recorder ─────────────────────────────────────────────────────────────
# Writes WAV files of the last ~10 s at each pipeline stage so you can open
# them in Audacity to hear exactly what happens at each step.
# Files land in ~/arctis_rvc_debug/ and are overwritten on each save cycle.

_DEBUG_WAVS = False       # master switch: set True to capture debug WAV stages
_DEBUG_DIR  = Path.home() / 'arctis_rvc_debug'
_DEBUG_SECS = 10.0        # rolling window length in seconds
_SAVE_EVERY = 16          # save to disk every N inference cycles (~2 s)


def _wav_write(path: Path, data: np.ndarray, sr: int) -> None:
    pcm = np.clip(data * 32767.0, -32768, 32767).astype(np.int16)
    with wave.open(str(path), 'wb') as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sr)
        wf.writeframes(pcm.tobytes())

_HUBERT_SR = 16000       # HuBERT input sample rate
_HUBERT_HOP = 320        # samples per feature at 16kHz → 50fps
_FEATURE_RATE = 100      # fps after doubling
_OUTPUT_SR = 48000       # fixed output rate; matches rvc_chain._PLAYBACK_RATE

_F0_MIN = 50.0           # Hz
_F0_MAX = 1100.0
_F0_MEL_MIN = 1127.0 * math.log(1.0 + _F0_MIN / 700.0)
_F0_MEL_MAX = 1127.0 * math.log(1.0 + _F0_MAX / 700.0)

# Sliding-window inference constants.
# Each inference uses a 512ms window (_WINDOW_FRAMES) but only advances by
# 128ms (_HOP_FRAMES = 2 input chunks).  The remaining 384ms (_CONTEXT_FRAMES)
# are real previous audio — no hard chunk boundaries, no click artifacts, no
# "reversed" quality from each window starting cold with zero-padded context.
_WINDOW_FRAMES  = 8192   # 512ms at 16kHz — full inference window
_HOP_FRAMES     = 2048   # 128ms at 16kHz — new samples advanced per inference
_CONTEXT_FRAMES = _WINDOW_FRAMES - _HOP_FRAMES   # 6144 = 384ms previous audio

# 320-sample pad forces HuBERT to produce 26 frames for an 8192-sample window
# instead of 25, so feature output (52 × 480 = 24960) exceeds the trimmed
# target (24576 = WINDOW_FRAMES × 3), avoiding an underrun.
_HUBERT_EXTRA_PAD = 320

# SOLA (synchronized overlap-add) hop stitching.
# Consecutive NSF synthesis runs have unrelated phase, so a hard cut at each
# hop boundary is a waveform discontinuity — audible as periodic crackle
# ("micro-stutter") on sustained vowels.  Blind crossfading instead causes
# phase-cancellation beats.  SOLA fixes both: find the offset (within a small
# search range) where the new hop's waveform best correlates with the existing
# tail, then crossfade at the aligned position.  Same approach as w-okada and
# the RVC realtime clients.
_XFADE_OUT   = 480    # 10 ms @ 48 kHz crossfade length
_SOLA_SEARCH = 960    # 20 ms @ 48 kHz alignment search range

# VAD gate: skip synthesis when neither the current hop nor the look-ahead
# carries speech.  Detection uses the look-ahead too so the gate opens one hop
# BEFORE speech arrives (no clipped word onsets), and a hangover keeps it open
# after speech ends (no chopped phrase tails).
# NC-gated silence is near digital zero, so the threshold can sit well below
# speech level.  Nasals/plosives at word onsets ('m', 'b') are the quietest
# phonemes (~0.002–0.004 RMS) — a higher threshold swallows them
# ("My name" → "mmm name").
_VAD_RMS       = 0.0015
_VAD_HANG_HOPS = 4    # keep gate open ~510 ms after last speech hop

# Relative VAD: a hop must also reach this fraction of the running speech
# level (peak-hold, slow decay) to count as speech.  Breath after a phrase is
# real acoustic energy above the absolute floor (~-46 dB) but 15–20 dB below
# speech; without this it holds the gate open and the RMS-normalised window
# drives the model to synthesise voice out of the breath — heard as random
# vocals/mumbling at every phrase end.  0.2 ≈ -14 dB relative: quiet
# word-onset nasals still pass because their look-ahead hop carries the word
# body (the gate opens on chunk OR look-ahead level).
_VAD_REL        = 0.2
# Speech-level tracker: instant attack, ~1.3 s release (10 speech hops)
# toward the current level, so a speaker dropping from loud to quiet isn't
# gated by a stale loud reference.  Only speech-classed hops update it.
_SPEECH_RMS_RELEASE = 0.9

# Voicedness rescue: a natural phrase-final syllable decays through the
# relative VAD threshold while still strongly periodic (measured ~0.7
# normalized autocorrelation on real tails), whereas breath/room noise sits
# at ~0.2–0.3.  A quiet-but-voiced chunk is speech — without this the
# relative VAD chops the last ~150 ms of word-final vowels ("Ginny" → "J'nn").
_VOICED_MIN = 0.45


def _voicedness(chunk: np.ndarray, sr: int) -> float:
    """Normalized autocorrelation peak in the 50–400 Hz pitch band (0..1)."""
    x = chunk - chunk.mean()
    n = len(x)
    if n < sr // 25 or float(np.dot(x, x)) < 1e-8:
        return 0.0
    f = np.fft.rfft(x, 2 * n)
    ac = np.fft.irfft(f * np.conj(f))[:n]
    lo, hi = sr // 400, min(sr // 50, n - 1)
    return float(ac[lo:hi].max() / (ac[0] + 1e-9))

# Output-gate envelope release time.  Must bridge plosive closures (50–120 ms
# of true silence inside words) yet close on real silence before the VAD
# hangover ends: 60 ms holds mask=1.0 through a 120 ms stop and reaches ~0 in
# ~280 ms of genuine silence.
_GATE_RELEASE_S = 0.060


def _fill_f0_gaps(f0: np.ndarray, max_gap: int = 3) -> np.ndarray:
    """Interpolate over short unvoiced flickers inside voiced runs.

    Weakly-voiced phonation (nasals, closed vowels, creak) makes RMVPE's
    voicing decision flicker on/off frame to frame; each off frame switches
    the NSF vocoder to noise excitation mid-vowel — audible as garbled rasp.
    Gaps of ≤ max_gap frames (30 ms) flanked by voiced frames are bridged by
    linear interpolation.  Genuine unvoiced consonants are longer and keep
    their gap.
    """
    v = np.flatnonzero(f0 > 0)
    if v.size < 2:
        return f0
    for a, b in zip(v[:-1], v[1:]):
        gap = b - a - 1
        if 0 < gap <= max_gap:
            f0[a + 1:b] = np.linspace(f0[a], f0[b], gap + 2)[1:-1]
    return f0


def _f0_to_coarse(f0: np.ndarray) -> np.ndarray:
    mel = 1127.0 * np.log(1.0 + np.maximum(f0, 1e-6) / 700.0)
    coarse = np.where(
        f0 > 0,
        np.clip((mel - _F0_MEL_MIN) * 254.0 / (_F0_MEL_MAX - _F0_MEL_MIN) + 1, 1, 255),
        0,
    )
    return np.rint(coarse).astype(np.int64)


def _extract_f0_autocorr(audio: np.ndarray, sr: int, hop: int,
                          f0_min: float = _F0_MIN,
                          f0_max: float = _F0_MAX) -> np.ndarray:
    """Frame-wise autocorrelation F0 extraction. Returns Hz array, 0 = unvoiced."""
    frame_len = hop * 4
    n_frames = max(1, len(audio) // hop)
    f0 = np.zeros(n_frames, dtype=np.float32)
    lag_min = int(sr / f0_max)
    lag_max = int(sr / f0_min)

    for i in range(n_frames):
        center = i * hop
        start = max(0, center - frame_len // 2)
        end = min(len(audio), start + frame_len)
        frame = audio[start:end].astype(np.float64)
        if len(frame) < lag_max + 1:
            continue
        frame -= frame.mean()
        energy = np.dot(frame, frame)
        if energy < 1e-6:
            continue
        # normalized autocorrelation via FFT
        n_fft = 1 << int(np.ceil(np.log2(len(frame) * 2)))
        spec = np.fft.rfft(frame, n=n_fft)
        acorr = np.fft.irfft(spec * spec.conj()).real[:len(frame)]
        acorr /= acorr[0] + 1e-10
        region = acorr[lag_min:lag_max + 1]
        if len(region) == 0:
            continue
        peak_idx = int(np.argmax(region))
        peak_val = region[peak_idx]
        if peak_val > 0.3:
            f0[i] = sr / (lag_min + peak_idx)

    return f0


class RVCPipeline:
    """HuBERT feature extraction + RVC VITS inference."""

    def __init__(self) -> None:
        self._hubert: torch.nn.Module | None = None
        self._model: torch.nn.Module | None = None
        self._rmvpe = None   # RMVPE instance, loaded lazily
        self._device: torch.device | None = None
        self._model_sr: int = 48000
        self._params: 'RVCParams | None' = None
        self._context_buf: np.ndarray = np.empty(0, dtype=np.float32)  # previous window tail
        self._new_buf: np.ndarray = np.empty(0, dtype=np.float32)      # accumulates until HOP
        self._out_buf: np.ndarray = np.empty(0, dtype=np.float32)
        self._vad_hang = 0            # hops of hangover left after last speech
        self._gate_was_open = False   # for fade-out on speech→silence transition
        # Running speech level (peak-hold with slow decay).  Breath and room
        # noise after a phrase sit well above the absolute _VAD_RMS floor but
        # far below actual speech; gating relative to this level keeps the
        # pipeline from synthesising "voice" out of a breath tail.
        self._speech_rms = 0.0
        # F0 continuity reference for the fry/octave-jump clamp; None until
        # the first voiced frame of an utterance, reset when the gate closes.
        self._f0_ref: float | None = None
        # Recent window-median voiced F0 values (post-offset), only from
        # strongly-voiced windows.  The pitch anchor for the phrase-final F0
        # floor is the median of this deque: robust against outlier windows
        # in BOTH directions (an EMA with fast attack got poisoned high by a
        # single keyboard-transient window and pitched the whole voice up).
        self._f0_meds: deque[float] = deque(maxlen=15)
        # Output-gate mask for the last _XFADE_OUT samples of the previous hop:
        # the SOLA crossfade re-synthesises that region, so it must be masked
        # with the PREVIOUS hop's envelope or silent boundaries click.
        self._prev_mask_tail = np.zeros(_XFADE_OUT, dtype=np.float32)
        self._env_last = 0.0          # gate-envelope release carry across hops
        # Previous hop's envelope tail, for SOLA-shifted mask sourcing
        self._env_tail = np.zeros(_SOLA_SEARCH * 16000 // _OUTPUT_SR, dtype=np.float32)
        self._ctx_frozen = False      # context held through silence (speech-only context)
        # Cold start: context is all silence right after the gate was closed.
        # The first hops of an utterance then synthesise from a mostly-silence
        # window → garbled short words.  At cold start we wait for a second
        # look-ahead hop (256 ms of real right context); the one-hop latency
        # debt is repaid by emitting fewer zeros during the next silence.
        self._cold_start = True
        # Debug rolling buffers: name → (list[chunk], total_samples, sample_rate)
        self._dbg_bufs:  dict[str, list[np.ndarray]] = {}
        self._dbg_total: dict[str, int] = {}
        self._dbg_sr:    dict[str, int] = {}
        self._dbg_cycle: int = 0
        # Rolling per-hop quality metrics for the auto-tuner (thread-safe:
        # written by the convert thread, drained by the D-Bus thread).
        self._metrics_lock = threading.Lock()
        self._metrics: deque[dict] = deque(maxlen=200)
        # FAISS feature-retrieval index (loaded with the model when a .index
        # file exists next to it; blend controlled by params.index_rate)
        self._faiss_index = None
        self._faiss_feats: np.ndarray | None = None
        if _DEBUG_WAVS:
            _DEBUG_DIR.mkdir(parents=True, exist_ok=True)
            logger.info('RVC debug buffers enabled — WAV files → %s', _DEBUG_DIR)

    # ── Public ────────────────────────────────────────────────────────────

    def load(self, path: Path, device: torch.device,
             params: 'RVCParams | None' = None) -> None:
        from linux_arctis_manager.voice_changer.rvc.backend import RVCParams
        self._device = device
        self._params = params or RVCParams()
        if self._params.hubert_model == 'contentvec':
            self._load_hubert_contentvec(device)
        else:
            self._load_hubert_torchaudio(device)
        self._load_synthesizer(path, device)
        self._context_buf = np.empty(0, dtype=np.float32)
        self._new_buf = np.empty(0, dtype=np.float32)
        # Seed with _XFADE_OUT zeros so the crossfade guard always passes in
        # steady state.  Without this, produce == drain exactly, _out_buf is
        # empty when each hop arrives, and the guard never fires.
        self._out_buf = np.zeros(_XFADE_OUT, dtype=np.float32)
        self._vad_hang = 0
        self._gate_was_open = False
        self._prev_mask_tail = np.zeros(_XFADE_OUT, dtype=np.float32)
        self._env_last = 0.0
        self._env_tail = np.zeros(_SOLA_SEARCH * 16000 // _OUTPUT_SR, dtype=np.float32)
        self._cold_start = True
        self._ctx_frozen = False
        self._load_rmvpe(device)
        self._load_faiss_index(path)
        logger.info('RVC pipeline ready on %s (model_sr=%d params=%s)',
                    device, self._model_sr, self._params)

    def _load_faiss_index(self, model_path: Path) -> None:
        """Load the model's FAISS feature index, if one exists next to it.

        Matching: '<stem>.index' first, then any '*.index' whose filename
        contains the model stem (RVC WebUI exports 'added_IVF…_<name>_v2.index').
        The training-set feature matrix is reconstructed once at load; each
        window's HuBERT features are then blended with their retrieved nearest
        neighbours (see _run_inference), pulling out-of-distribution input
        (e.g. creaky phrase endings) toward the model's training distribution.
        """
        self._faiss_index = None
        self._faiss_feats = None
        # Load whenever a file exists (not only when index_rate > 0) so the
        # rate can be raised live via update_params without a chain rebuild.
        stem = model_path.stem
        folder = model_path.parent
        candidates = ([folder / f'{stem}.index'] +
                      sorted(p for p in folder.glob('*.index') if stem in p.stem))
        index_path = next((p for p in candidates if p.is_file()), None)
        if index_path is None:
            logger.info('RVC index: none found for %r — retrieval disabled', stem)
            return
        try:
            import faiss
            index = faiss.read_index(str(index_path))
            feats = index.reconstruct_n(0, index.ntotal)
            self._faiss_index = index
            self._faiss_feats = feats
            logger.info('RVC index loaded: %s (%d vectors, %.0f MB)',
                        index_path.name, index.ntotal, feats.nbytes / 1e6)
        except ImportError:
            logger.warning('RVC index found but faiss is not installed — retrieval disabled')
        except Exception as e:
            logger.error('RVC index load failed (%s): %s', index_path.name, e)

    def update_params(self, params: 'RVCParams') -> None:
        """Swap tuning params live — safe because _params is read once per hop."""
        self._params = params
        logger.info('RVC params updated live: %s', params)

    def drain_metrics(self) -> dict:
        """Return aggregated per-hop quality metrics collected since the last
        call, then clear the buffer.  Single-consumer (the auto-tuner)."""
        with self._metrics_lock:
            hops = list(self._metrics)
            self._metrics.clear()
        speaking = [h for h in hops if h['speaking']]
        return {
            'hops':          len(hops),
            'speaking_hops': len(speaking),
            'sat_ratio':     float(np.mean([h['sat'] for h in speaking])) if speaking else 0.0,
            'peak':          float(max((h['peak'] for h in speaking), default=0.0)),
            'in_rms':        float(np.mean([h['in_rms'] for h in speaking])) if speaking else 0.0,
        }

    def unload(self) -> None:
        self._dbg_flush()   # save whatever is in the rolling buffers before teardown
        self._hubert = None
        self._model = None
        try:
            import torch as _torch
            _torch.cuda.empty_cache()
        except Exception:
            pass
        self._rmvpe = None
        self._faiss_index = None
        self._faiss_feats = None
        self._context_buf = np.empty(0, dtype=np.float32)
        self._new_buf = np.empty(0, dtype=np.float32)
        self._out_buf = np.zeros(_XFADE_OUT, dtype=np.float32)
        self._vad_hang = 0
        self._gate_was_open = False
        self._prev_mask_tail = np.zeros(_XFADE_OUT, dtype=np.float32)
        self._env_last = 0.0
        self._env_tail = np.zeros(_SOLA_SEARCH * 16000 // _OUTPUT_SR, dtype=np.float32)
        self._cold_start = True
        self._ctx_frozen = False

    def convert(self, audio: np.ndarray, sr: int, pitch_offset: float) -> np.ndarray:
        """
        audio:        float32 numpy [-1,1] at sr (typically 16000)
        sr:           input sample rate
        pitch_offset: semitones (positive = higher)
        returns:      float32 numpy at model_sr (48000), model_sr/sr × longer than input

        Uses a sliding window: each inference covers _WINDOW_FRAMES of audio
        but advances only _HOP_FRAMES.  The overlapping _CONTEXT_FRAMES are
        real previous audio — eliminates click artifacts and cold-start distortion
        at chunk boundaries.
        """
        n_out = len(audio) * _OUTPUT_SR // sr

        if self._model is None or self._device is None:
            return np.zeros(n_out, dtype=np.float32)

        # Stage 1: raw NC-mic chunks entering the pipeline (16 kHz)
        self._dbg_push('01_raw_input_16k', audio, sr)

        self._new_buf = np.concatenate([self._new_buf, audio])

        # Need current hop + look-ahead so _run_inference gets real future
        # audio as right context instead of an artificial reflect/zero pad.
        # Steady state: 1 look-ahead hop (+128 ms latency).  Cold start (first
        # hop after silence): 2 look-ahead hops, because the left context is
        # all silence and short words otherwise synthesise as garbage; the
        # extra hop of latency is repaid during the next silence.
        while True:
            required = (3 if self._cold_start else 2) * _HOP_FRAMES
            if len(self._new_buf) < required:
                break
            new_chunk = self._new_buf[:_HOP_FRAMES]
            look_ahead = self._new_buf[_HOP_FRAMES:required]
            self._new_buf = self._new_buf[_HOP_FRAMES:]

            # VAD gate: silence → zeros, no inference.  Keeps context current
            # so speech onset has proper history when gate reopens.  The
            # look-ahead is checked too, opening the gate one hop early so
            # word onsets aren't clipped; the hangover keeps it open after
            # speech so quiet phrase tails aren't chopped.
            chunk_rms = float(np.sqrt(np.mean(new_chunk ** 2)))
            la_rms = float(np.sqrt(np.mean(look_ahead ** 2)))
            vad_thr = max(_VAD_RMS, _VAD_REL * self._speech_rms)
            level_ok = chunk_rms >= vad_thr or la_rms >= vad_thr
            # Quiet-but-voiced chunks (decaying phrase-final vowels) are
            # speech; only test when the level check fails and the chunk is
            # above the absolute floor.  A phrase tail by definition follows
            # speech — requiring the gate to have been open keeps resonant
            # transients in silence (keyboard clicks, paper rustle, whose
            # mechanical ring can pass the periodicity test) from opening
            # the gate and being synthesised as vocal blips.
            voiced_ok = (not level_ok and self._gate_was_open
                         and chunk_rms >= _VAD_RMS
                         and _voicedness(new_chunk, sr) >= _VOICED_MIN)
            if level_ok or voiced_ok:
                if level_ok:
                    # Track only hops near the current speech level: rescued
                    # voiced tails and marginal fade-out hops sit far below it
                    # by definition, and letting them release the tracker
                    # drops the threshold under breath level by phrase end.
                    cur = max(chunk_rms, la_rms)
                    if cur > self._speech_rms:
                        self._speech_rms = cur
                    elif cur >= 0.5 * self._speech_rms:
                        self._speech_rms = (_SPEECH_RMS_RELEASE * self._speech_rms
                                            + (1 - _SPEECH_RMS_RELEASE) * cur)
                self._vad_hang = _VAD_HANG_HOPS
                gate_open = True
                hangover_hop = False
            elif self._vad_hang > 0:
                self._vad_hang -= 1
                gate_open = True
                hangover_hop = True
            else:
                gate_open = False
                hangover_hop = False
            if not gate_open:
                with self._metrics_lock:
                    self._metrics.append(
                        {'speaking': False, 'sat': 0.0, 'peak': 0.0, 'in_rms': chunk_rms})
                n_hop_out = _HOP_FRAMES * _OUTPUT_SR // sr
                # Fade the reserve tail to zero on the speech→silence edge so
                # the transition into digital silence doesn't click.
                if self._gate_was_open and len(self._out_buf) >= _XFADE_OUT:
                    self._out_buf[-_XFADE_OUT:] *= np.linspace(
                        1.0, 0.0, _XFADE_OUT, dtype=np.float32)
                self._gate_was_open = False
                self._cold_start = True
                self._f0_ref = None
                self._prev_mask_tail = np.zeros(_XFADE_OUT, dtype=np.float32)
                # Keep the release envelope decaying through skipped hops
                self._env_last *= math.exp(-_HOP_FRAMES / (_GATE_RELEASE_S * sr))
                self._env_tail = np.full_like(self._env_tail, self._env_last)
                # Repay latency debt from cold-start stalls: any backlog in
                # _out_buf beyond the reserve + this call's drain is delayed
                # audio; emit fewer silence zeros so the timing pulls back.
                excess = max(0, len(self._out_buf) - _XFADE_OUT - n_out)
                zeros_n = max(0, n_hop_out - excess)
                self._out_buf = np.concatenate([
                    self._out_buf, np.zeros(zeros_n, dtype=np.float32)
                ])
                # Context is FROZEN through silence — do not slide zeros in.
                # Sliding silence through meant every post-pause word was
                # synthesised from a silence-dominated window (garbled), while
                # the same word mid-phrase rode on real speech context and
                # sounded fine.  Freezing makes a post-pause word look like
                # continuous speech to the (stateless) model.
                self._ctx_frozen = True
                continue
            self._gate_was_open = True

            # Unfreeze: smooth the splice between the pre-pause speech held in
            # the frozen context and the new phrase (5 ms fade-out on the old
            # tail — reads as a natural glottal stop, not a click).
            if self._ctx_frozen and len(self._context_buf) >= 80:
                self._context_buf[-80:] *= np.linspace(1.0, 0.0, 80, dtype=np.float32)
            self._ctx_frozen = False

            # Zero-pad context only before the pipeline is warm
            ctx = self._context_buf
            if len(ctx) < _CONTEXT_FRAMES:
                ctx = np.concatenate([
                    np.zeros(_CONTEXT_FRAMES - len(ctx), dtype=np.float32), ctx
                ])
            window = np.concatenate([ctx, new_chunk])   # WINDOW_FRAMES samples

            # Stage 2: the full sliding window fed to the synthesizer (16 kHz)
            self._dbg_push('02_window_16k', window, sr)

            full_out = self._run_inference(window, sr, pitch_offset, look_ahead)
            n_hop_out = _HOP_FRAMES * _OUTPUT_SR // sr

            # Take extra samples from the context region: crossfade length plus
            # the SOLA search range, so alignment can slide up to _SOLA_SEARCH.
            n_take = n_hop_out + _XFADE_OUT + _SOLA_SEARCH
            if len(full_out) >= n_take:
                hop_ext = full_out[-n_take:]
            else:
                hop_ext = np.pad(full_out, (0, n_take - len(full_out)))

            # Quality metrics, measured pre-limiter: sat = fraction of samples
            # riding the generator's tanh rail (audible harmonic distortion).
            with self._metrics_lock:
                self._metrics.append({
                    'speaking': True,
                    'sat':      float(np.mean(np.abs(hop_ext) > 0.95)),
                    'peak':     float(np.abs(hop_ext).max()),
                    'in_rms':   chunk_rms,
                })

            # Per-hop soft limiter: linear below the knee, tanh-compressed above.
            # Default 0.80 catches true clipping peaks without compressing the
            # bulk of the dynamic range; tunable per model (1.0 disables).
            _SOFT_THR = self._params.limiter_thr if self._params else 0.80
            _OUT_CEIL = 1.0
            if _SOFT_THR < 0.999:
                hop_ext = np.where(
                    np.abs(hop_ext) <= _SOFT_THR,
                    hop_ext,
                    np.sign(hop_ext) * (
                        _SOFT_THR + (_OUT_CEIL - _SOFT_THR) *
                        np.tanh((np.abs(hop_ext) - _SOFT_THR) / (_OUT_CEIL - _SOFT_THR))
                    ),
                ).astype(np.float32)

            # SOLA: find the offset (0.._SOLA_SEARCH) where the new output's
            # waveform best aligns with the un-drained tail of _out_buf, then
            # crossfade there.  Normalised cross-correlation avoids loudness
            # bias.  Timing is preserved: exactly n_hop_out samples are
            # appended regardless of the chosen offset.
            tail = self._out_buf[-_XFADE_OUT:] if len(self._out_buf) >= _XFADE_OUT else None
            if tail is not None and np.any(tail):
                seg = hop_ext[:_XFADE_OUT + _SOLA_SEARCH]
                corr = np.correlate(seg, tail, 'valid')
                norm = np.sqrt(
                    np.convolve(seg.astype(np.float64) ** 2,
                                np.ones(_XFADE_OUT), 'valid') + 1e-8)
                k = int(np.argmax(corr / norm))
            else:
                k = 0
            aligned = hop_ext[k:]
            hop = aligned[_XFADE_OUT:_XFADE_OUT + n_hop_out]

            # Input-envelope output gate, at 10 ms resolution.  The model
            # synthesises a noise floor for silent input (NSF unvoiced noise
            # excitation + VITS prior noise — silence is out-of-distribution
            # for speech-trained models).  The hop-level VAD can't catch the
            # short gaps between words/syllables or the hangover hops, so gate
            # every output sample with the smoothed input envelope of the SAME
            # time window: silence in → silence out, speech passes untouched.
            env = np.convolve(np.abs(new_chunk),
                              np.ones(160, dtype=np.float32) / 160, 'same')
            # Fast attack, _GATE_RELEASE_S release.  Plosive closures ('p',
            # 'pp') are genuine 50–120 ms silences inside words; an instantly-
            # tracking mask chops the stop plus the quiet unstressed syllable
            # after it ("clipping anymore" → "clip… nymore").  The release
            # bridges intra-word stops (mask stays 1.0 through a 120 ms
            # closure) while real silence still gates to ~0 within ~280 ms,
            # inside the VAD hangover.  env_rel[i] = max(env[j]·decay^(i−j)),
            # carried across hops via _env_last.
            decay = math.exp(-1.0 / (_GATE_RELEASE_S * sr))
            dk = decay ** np.arange(len(env) + 1)
            a = np.concatenate(([self._env_last], env)) / dk
            env_rel = (dk * np.maximum.accumulate(a))[1:].astype(np.float32)
            self._env_last = float(env_rel[-1])

            # Align the mask with the SOLA time shift: the hop content lags
            # input time by (_SOLA_SEARCH − k) output samples, so an unshifted
            # mask lands up to 20 ms early and shaves attacks at onsets and
            # after stops.  Use the previous hop's envelope tail to source the
            # shifted region.
            n_tail = _SOLA_SEARCH * sr // _OUTPUT_SR
            shift = (_SOLA_SEARCH - k) * sr // _OUTPUT_SR
            ext = np.concatenate([self._env_tail, env_rel])
            start = n_tail - shift
            env_shifted = ext[start : start + len(env_rel)]
            self._env_tail = env_rel[-n_tail:].copy()

            # Speech hops keep the gentle absolute knee (quiet word-onset
            # nasals must pass).  Hangover hops carry sub-speech input —
            # breath, room noise — that the model happily voices; raising the
            # knee to half the running speech level crushes that hallucinated
            # tail while a genuinely quiet phrase tail (still speech-classed
            # by the relative VAD) is untouched.
            knee = 0.002 if not hangover_hop else max(0.002, 0.5 * self._speech_rms)
            mask = np.clip(env_shifted / knee, 0.0, 1.0) ** 2
            mask_out = np.repeat(mask, _OUTPUT_SR // sr)
            if len(mask_out) < n_hop_out:
                mask_out = np.pad(mask_out, (0, n_hop_out - len(mask_out)), mode='edge')
            mask_out = mask_out[:n_hop_out].astype(np.float32)
            hop = (hop * mask_out).astype(np.float32)

            # Stage 5: the gated hop sent to pacat (48 kHz)
            self._dbg_push('05_hop_output_48k', hop, _OUTPUT_SR)

            if tail is not None:
                fade_out = np.linspace(1.0, 0.0, _XFADE_OUT, dtype=np.float32)
                fade_in  = 1.0 - fade_out
                # The crossfade region re-synthesises the tail of the PREVIOUS
                # hop, so apply the previous hop's gate mask to it — blending
                # unmasked audio into a masked tail injects a 10 ms noise
                # burst at every hop boundary (audible as a rhythmic click).
                self._out_buf[-_XFADE_OUT:] = (
                    tail * fade_out +
                    aligned[:_XFADE_OUT] * self._prev_mask_tail * fade_in
                )
            self._out_buf = np.concatenate([self._out_buf, hop])
            self._prev_mask_tail = mask_out[-_XFADE_OUT:]
            self._cold_start = False

            # Slide context forward — but only with chunks that contain actual
            # speech (including voiced-rescued quiet tails).  Hangover hops
            # carry near-silence; letting them into the context would dilute
            # it before the next freeze.
            if chunk_rms >= vad_thr or voiced_ok:
                self._context_buf = np.concatenate([self._context_buf, new_chunk])
                if len(self._context_buf) > _CONTEXT_FRAMES:
                    self._context_buf = self._context_buf[-_CONTEXT_FRAMES:]
            else:
                self._ctx_frozen = True

            # Flush debug WAVs to disk every _SAVE_EVERY inference cycles
            self._dbg_cycle += 1
            if self._dbg_cycle % _SAVE_EVERY == 0:
                self._dbg_flush()

        # Keep _XFADE_OUT samples in _out_buf as a reserve so the retroactive
        # crossfade guard (len >= _XFADE_OUT) always passes in steady state.
        if len(self._out_buf) - _XFADE_OUT >= n_out:
            out = self._out_buf[:n_out]
            self._out_buf = self._out_buf[n_out:]
            return out.astype(np.float32)
        return np.zeros(n_out, dtype=np.float32)

    # ── Internals ─────────────────────────────────────────────────────────

    # ── Debug helpers ─────────────────────────────────────────────────────────

    def _dbg_push(self, name: str, data: np.ndarray, sr: int) -> None:
        if not _DEBUG_WAVS:
            return
        if name not in self._dbg_bufs:
            self._dbg_bufs[name] = []
            self._dbg_total[name] = 0
            self._dbg_sr[name] = sr
        self._dbg_bufs[name].append(data.astype(np.float32))
        self._dbg_total[name] += len(data)
        max_samples = int(sr * _DEBUG_SECS)
        while self._dbg_total[name] > max_samples and len(self._dbg_bufs[name]) > 1:
            dropped = self._dbg_bufs[name].pop(0)
            self._dbg_total[name] -= len(dropped)

    def _dbg_flush(self) -> None:
        if not _DEBUG_WAVS:
            return
        for name, chunks in self._dbg_bufs.items():
            if not chunks:
                continue
            data = np.concatenate(chunks)
            _wav_write(_DEBUG_DIR / f'{name}.wav', data, self._dbg_sr[name])

    def _load_rmvpe(self, device: torch.device) -> None:
        from linux_arctis_manager.voice_changer.rvc.rmvpe import RMVPE
        try:
            self._rmvpe = RMVPE(device)
        except Exception as e:
            logger.warning('RMVPE unavailable, falling back to autocorrelation F0: %s', e)
            self._rmvpe = None

    def _load_hubert_torchaudio(self, device: torch.device) -> None:
        import torchaudio
        bundle = torchaudio.pipelines.HUBERT_BASE
        self._hubert = bundle.get_model().to(device).eval()
        logger.info('HuBERT (torchaudio) loaded on %s', device)

    def _load_hubert_contentvec(self, device: torch.device) -> None:
        import torchaudio
        path = _ensure_contentvec()
        ckpt = torch.load(path, map_location='cpu', weights_only=False)
        # fairseq checkpoint wraps weights under 'model' key
        raw = ckpt.get('model', ckpt)
        logger.info('ContentVec raw keys (first 5): %s', list(raw.keys())[:5])
        mapped = _remap_contentvec(raw)
        bundle = torchaudio.pipelines.HUBERT_BASE
        model = bundle.get_model()
        total_params = len(list(model.state_dict().keys()))
        missing, unexpected = model.load_state_dict(mapped, strict=False)
        loaded = total_params - len(missing)
        logger.info('ContentVec load: %d/%d params loaded, %d missing, %d unexpected',
                    loaded, total_params, len(missing), len(unexpected))
        if missing:
            logger.warning('ContentVec missing keys (first 5): %s', missing[:5])
        model.eval().to(device)
        self._hubert = model
        logger.info('HuBERT (ContentVec) loaded on %s', device)

    def _load_synthesizer(self, path: Path, device: torch.device) -> None:
        import torch as _torch
        ckpt = _torch.load(path, map_location='cpu', weights_only=False)
        logger.info('Checkpoint keys: %s', list(ckpt.keys()))
        version = ckpt.get('version', 'v1')
        logger.info('Checkpoint version=%r  f0=%r  sr=%r  info=%r',
                    version, ckpt.get('f0'), ckpt.get('sr'),
                    ckpt.get('info', ckpt.get('epoch_info', '—')))
        if version != 'v2':
            raise ValueError(
                f'Model {path.name!r} is RVC {version}; only v2 (768-dim) is supported. '
                'Re-download or retrain with v2.'
            )
        config: list = ckpt['config']
        weights: dict = ckpt['weight']
        self._model_sr = _parse_sr(ckpt.get('sr', config[-1]))

        from linux_arctis_manager.voice_changer.rvc.synth import SynthesizerTrnMs768NSFsid
        model = SynthesizerTrnMs768NSFsid(*config)
        missing, unexpected = model.load_state_dict(weights, strict=False)
        if missing:
            logger.debug('RVC state_dict: %d missing keys (enc_q expected)', len(missing))
        if unexpected:
            logger.warning('RVC state_dict: %d unexpected keys', len(unexpected))
        model.eval().half().to(device)
        self._model = model
        logger.info('Synthesizer loaded from %s', path.name)

    def _extract_features(self, audio: np.ndarray) -> np.ndarray:
        assert self._hubert is not None
        assert self._device is not None
        # Pad so HuBERT's CNN produces 26 frames for an 8192-sample window
        # instead of 25 (see _HUBERT_EXTRA_PAD comment above).
        audio_padded = np.concatenate([audio, np.zeros(_HUBERT_EXTRA_PAD, dtype=np.float32)])
        wav = torch.from_numpy(audio_padded).float().unsqueeze(0).to(self._device)
        with torch.inference_mode():
            layers, _ = self._hubert.extract_features(wav)
        feats = layers[-1].squeeze(0).cpu().numpy()  # [T, 768], final transformer layer
        feats = np.repeat(feats, 2, axis=0)          # 50fps → 100fps
        return feats

    def _extract_f0(self, audio: np.ndarray, sr: int,
                    pitch_offset: float) -> tuple[np.ndarray, np.ndarray]:
        if self._rmvpe is not None:
            # 0.022 instead of RMVPE's default 0.03: nasal and closed vowels
            # (e.g. British English) have weak harmonics — anti-formants
            # cancel them — and sit just under the default threshold, getting
            # synthesised as noise excitation.  Input is NC-cleaned, so the
            # lower threshold doesn't pick up environmental noise.
            f0 = self._rmvpe.infer(audio, threshold=0.022)
        else:
            hop = sr // _FEATURE_RATE   # 160 at 16kHz → 100fps
            f0 = _extract_f0_autocorr(audio, sr, hop)
        # max_gap 8 (80 ms): phrase-final creaky voice makes RMVPE drop 50–80 ms
        # of a voiced nasal mid-word ("Ginny") to unvoiced; the NSF then
        # switches to noise excitation in the middle of the word (heard as a
        # mumble).  Interpolating across the gap keeps the word voiced; true
        # stop closures are unaffected audibly because the HuBERT features
        # still encode the stop.
        f0 = _fill_f0_gaps(f0, max_gap=8)
        if pitch_offset != 0.0:
            voiced = f0 > 0
            f0[voiced] *= 2.0 ** (pitch_offset / 12.0)

        # Speaker-relative F0 floor.  Phrase-final declination + fry drives the
        # (post-offset) target F0 well below anything the model saw in
        # training (~0.5× the speaker's median), and the NSF renders those
        # frames as irregular pulses — random vocals/mumbling on phrase
        # endings.  Flooring at 0.8× the running median F0 keeps the tail
        # in-distribution; perceptually fry is quasi-static anyway.
        voiced = f0[f0 > 0]
        # Anchor updates only from strongly-voiced windows (a transient or a
        # phrase-tail window has few voiced frames and a junk median).
        if voiced.size >= 20:
            self._f0_meds.append(float(np.median(voiced)))
        # Apply the floor only once there is enough evidence of the modal
        # pitch.  0.9× the modal median: tight enough to keep the NSF stable
        # on fry (which lives at ~0.6–0.7× modal), loose enough that normal
        # declination inside phrases is untouched most of the time.
        # Perceptually fry is quasi-static, so pinning it is inaudible.
        if voiced.size and len(self._f0_meds) >= 5:
            anchor = float(np.median(self._f0_meds))
            floor = max(55.0, 0.9 * anchor)
            f0 = np.where((f0 > 0) & (f0 < floor), floor, f0)

        coarse = _f0_to_coarse(f0)
        return f0.astype(np.float32), coarse

    def _run_inference(self, audio: np.ndarray, sr: int,
                       pitch_offset: float,
                       look_ahead: np.ndarray | None = None) -> np.ndarray:
        assert self._model is not None
        assert self._device is not None

        # DC removal + RMS normalise to a target level.
        # Boosting quiet signals improves RMVPE F0 detection and HuBERT feature
        # quality.  Gain is capped at +12 dB to avoid blowing up true silence.
        # Peak-normalise instead of hard-clipping: flat-top clipping at ±0.95
        # added harmonic distortion that corrupted HuBERT features and made
        # synthesis sound harsh regardless of output-side processing.
        audio = audio - audio.mean()
        # Active-frame RMS: measure level over speech frames only, not the
        # whole window.  At a phrase onset the 640 ms window is mostly silence;
        # whole-window RMS is diluted ~10×, the gain hits its cap, and the
        # onset reaches the model underdriven — audible as mumbled first
        # syllables ("My name" → "mmm name").
        _frames = audio[:len(audio) // 160 * 160].reshape(-1, 160)
        _fr_rms = np.sqrt(np.mean(_frames ** 2, axis=1))
        _active = _fr_rms[_fr_rms >= max(float(_fr_rms.max()) * 0.2, 1e-5)]
        rms = float(np.mean(_active)) if _active.size else float(np.sqrt(np.mean(audio ** 2)))
        # Default 0.06 keeps the model well below tanh saturation; 0.10 drove
        # higher-amplitude models into the NSF generator's tanh causing audible
        # harmonic distortion on every voiced frame.  Tunable per model.
        _TARGET_RMS = self._params.target_rms if self._params else 0.06
        if rms > 1e-4:
            norm_gain = min(_TARGET_RMS / rms, 4.0)
            audio = audio * norm_gain
        else:
            norm_gain = 1.0
        peak = float(np.abs(audio).max())
        if peak > 0.70:
            norm_gain = norm_gain * (0.70 / peak)
            audio = audio * (0.70 / peak)
        audio = audio.astype(np.float32)

        # Right context: use real look-ahead audio when available (caller buffers
        # one extra hop before invoking inference).  Real future audio is in-
        # distribution for HuBERT (trained on full bidirectional context); a
        # reflected copy creates an unnatural mirror phoneme sequence that can
        # degrade features at the new-chunk boundary where we extract the hop.
        # Fall back to zero-pad (silence) when no look-ahead is provided — silence
        # is also in-distribution (standard batch-training padding), unlike reflect.
        # Apply the same DC+gain so the concatenated signal is level-consistent.
        # Look-ahead is 1 hop in steady state, 2 hops at cold start (the extra
        # right context is what keeps short words from garbling after silence).
        if look_ahead is not None and len(look_ahead) >= _HOP_FRAMES:
            la = look_ahead.astype(np.float32)
            la = (la - la.mean()) * norm_gain
            right_pad = la
        else:
            right_pad = np.zeros(_HOP_FRAMES, dtype=np.float32)
        audio_padded = np.concatenate([audio, right_pad])

        # Stage 3: normalized original window portion only (for audible diagnosis)
        self._dbg_push('03_normalized_16k', audio, sr)

        # VTLN: warp audio in frequency domain before HuBERT so formants shift
        # toward the model's training distribution (alpha<1 = upward shift for
        # male→female).  F0 is extracted from the original, unwarped audio so
        # pitch is unaffected.
        vtln_alpha = self._params.vtln_alpha if self._params else 1.0
        hubert_input = _vtln_warp(audio_padded, vtln_alpha)
        feats = self._extract_features(hubert_input)              # [T_f, 768]

        # FAISS retrieval blend (RVC WebUI 'index rate'): replace each feature
        # frame with a distance-weighted mix of its k nearest training-set
        # features.  Out-of-distribution input (creaky phrase endings,
        # transients) snaps to the nearest clean training features, which the
        # synthesizer renders reliably.
        index_rate = self._params.index_rate if self._params else 0.0
        if index_rate > 0.0 and self._faiss_index is not None and self._faiss_feats is not None:
            q = np.ascontiguousarray(feats, dtype=np.float32)
            score, ix = self._faiss_index.search(q, k=8)
            weight = np.square(1.0 / np.maximum(score, 1e-6))
            weight /= weight.sum(axis=1, keepdims=True)
            retrieved = np.sum(self._faiss_feats[ix] * np.expand_dims(weight, axis=2), axis=1)
            feats = (index_rate * retrieved + (1.0 - index_rate) * feats).astype(feats.dtype)

        f0, f0_coarse = self._extract_f0(audio_padded, sr, pitch_offset)

        # F0 median filter (WebUI 'filter_radius'): kills single-frame pitch
        # spikes that produce glottal bursts / crackle in the NSF source.
        radius = self._params.filter_radius if self._params else 3
        if radius >= 3 and len(f0) >= radius:
            radius |= 1  # ensure odd
            pad = radius // 2
            padded = np.pad(f0, pad, mode='edge')
            windows = np.lib.stride_tricks.sliding_window_view(padded, radius)
            f0_smooth = np.median(windows, axis=1).astype(f0.dtype)
            # Don't let the median smear voiced F0 into unvoiced (zero) frames
            f0 = np.where(f0 > 0, np.where(f0_smooth > 0, f0_smooth, f0), 0.0)

        # Continuity clamp: phrase-final vocal fry (and octave errors) make the
        # tracker emit wild frame-to-frame jumps (measured 84→326→500 Hz inside
        # one creaky word ending) that the NSF source renders as random vocals/
        # mumbling.  Real pitch never moves half an octave in 10 ms: any voiced
        # frame further than that from the running reference is pulled back to
        # it.  The reference tracks accepted frames (80/20 EMA), so legitimate
        # fast intonation still passes; it carries across the overlapping
        # windows via self._f0_ref and resets when the VAD gate closes.
        ref = self._f0_ref
        for i in range(len(f0)):
            if f0[i] <= 0:
                continue
            if ref is None:
                ref = float(f0[i])
            elif abs(math.log2(f0[i] / ref)) > 0.5:
                f0[i] = ref
            ref = 0.8 * ref + 0.2 * float(f0[i])
        self._f0_ref = ref
        f0_coarse = _f0_to_coarse(f0)

        voiced = f0[f0 > 0]
        if voiced.size:
            logger.debug('F0: voiced=%d/%d  min=%.0f median=%.0f max=%.0f Hz',
                         voiced.size, f0.size,
                         float(voiced.min()), float(np.median(voiced)), float(voiced.max()))

        # Align F0 length to feature length
        T_f = feats.shape[0]
        if len(f0) != T_f:
            f0 = _resize1d(f0, T_f)
            f0_coarse = _resize1d(f0_coarse.astype(np.float32), T_f).astype(np.int64)

        dev = self._device
        phone = torch.from_numpy(feats).half().unsqueeze(0).to(dev)      # [1,T,768]
        phone_len = torch.LongTensor([T_f]).to(dev)
        pitch = torch.from_numpy(f0_coarse).long().unsqueeze(0).to(dev)  # [1,T]
        pitchf = torch.from_numpy(f0).half().unsqueeze(0).to(dev)        # [1,T]
        sid = torch.LongTensor([0]).to(dev)

        with torch.inference_mode():
            audio_out = self._model.infer(phone, phone_len, pitch, pitchf, sid)

        out_np = audio_out.squeeze().float().cpu().numpy()   # float32, at model_sr
        out_np = np.nan_to_num(out_np, nan=0.0, posinf=0.0, neginf=0.0)
        # No hard clip here: model's own tanh already bounds to [-1, 1]; clipping
        # before the soft limiter in convert() would create flat-top artefacts.

        # Trim at model_sr to strip right-pad output.
        n_out_model = len(audio) * self._model_sr // sr
        if len(out_np) > n_out_model:
            out_np = out_np[:n_out_model]
        elif len(out_np) < n_out_model:
            out_np = np.pad(out_np, (0, n_out_model - len(out_np)))

        # Resample from model_sr to the fixed 48 kHz output rate.
        # 40 kHz models exist in the wild; without this the hop buffer fills at
        # 83 % of the rate pacat drains → underrun after ~1 s.
        out_np = _resample_audio(out_np, self._model_sr, _OUTPUT_SR)

        # Trim/pad to exact expected length at output rate.
        n_out_target = len(audio) * _OUTPUT_SR // sr
        if len(out_np) > n_out_target:
            out_np = out_np[:n_out_target]
        elif len(out_np) < n_out_target:
            out_np = np.pad(out_np, (0, n_out_target - len(out_np)))

        out_np = out_np.astype(np.float32)

        # Envelope mix: pull the model's hot sustained output level back toward
        # the input dynamics.  Directly reduces the perceived clipping — the
        # model tends to synthesise near-constant high amplitude regardless of
        # how loudly the source was speaking.
        rms_rate = self._params.rms_mix_rate if self._params else 1.0
        out_np = _mix_rms(audio, out_np, rms_rate)

        # Stage 4: synthesizer output for the original window (at _OUTPUT_SR)
        self._dbg_push('04_synth_output_48k', out_np, _OUTPUT_SR)

        return out_np


_CONTENTVEC_URL  = ('https://huggingface.co/lengyue233/content-vec-best/resolve/main/'
                    'pytorch_model.bin')
_CONTENTVEC_PATH = Path.home() / '.config' / 'arctis_manager' / 'models' / 'contentvec_500.bin'


def _ensure_contentvec() -> Path:
    _CONTENTVEC_PATH.parent.mkdir(parents=True, exist_ok=True)
    if not _CONTENTVEC_PATH.exists():
        logger.info('Downloading ContentVec encoder (~360 MB) — one-time setup...')
        tmp = _CONTENTVEC_PATH.with_suffix('.tmp')
        try:
            try:
                from huggingface_hub import hf_hub_download
                local = hf_hub_download(
                    repo_id='lengyue233/content-vec-best',
                    filename='pytorch_model.bin',
                    local_dir=str(_CONTENTVEC_PATH.parent),
                )
                import shutil
                shutil.move(local, _CONTENTVEC_PATH)
            except ImportError:
                import urllib.request
                urllib.request.urlretrieve(_CONTENTVEC_URL, tmp)
                tmp.rename(_CONTENTVEC_PATH)
        except Exception:
            tmp.unlink(missing_ok=True)
            raise
        logger.info('ContentVec saved to %s', _CONTENTVEC_PATH)
    return _CONTENTVEC_PATH


def _remap_fairseq_to_torchaudio(fs: dict) -> dict:
    """Remap fairseq HuBERT/ContentVec state dict keys to torchaudio Wav2Vec2 layout."""
    t: dict = {}

    # ── Feature extractor (CNN) ───────────────────────────────────────────────
    for i in range(7):
        p = f'feature_extractor.conv_layers.{i}'
        w = fs.get(f'{p}.0.weight')
        if w is not None:
            t[f'{p}.conv.weight'] = w
        lw = fs.get(f'{p}.2.weight')   # GroupNorm (layer 0) or LayerNorm
        lb = fs.get(f'{p}.2.bias')
        if lw is not None:
            t[f'{p}.layer_norm.weight'] = lw
        if lb is not None:
            t[f'{p}.layer_norm.bias'] = lb

    # ── Feature projection ────────────────────────────────────────────────────
    # fairseq "layer_norm" before post_extract_proj → torchaudio feature_projection.layer_norm
    for suffix in ('weight', 'bias'):
        v = fs.get(f'layer_norm.{suffix}')
        if v is not None:
            t[f'encoder.feature_projection.layer_norm.{suffix}'] = v
    for suffix in ('weight', 'bias'):
        v = fs.get(f'post_extract_proj.{suffix}')
        if v is not None:
            t[f'encoder.feature_projection.projection.{suffix}'] = v

    # ── Positional conv (weight-normed in fairseq) ────────────────────────────
    if 'encoder.pos_conv.0.weight_g' in fs and 'encoder.pos_conv.0.weight_v' in fs:
        wg = fs['encoder.pos_conv.0.weight_g']   # [512, 1, 1]
        wv = fs['encoder.pos_conv.0.weight_v']   # [512, 512, 128]
        norm = wv.norm(dim=[1, 2], keepdim=True).clamp(min=1e-7)
        t['encoder.transformer.pos_conv_embed.conv.weight'] = wg * wv / norm
    elif 'encoder.pos_conv.0.weight' in fs:
        t['encoder.transformer.pos_conv_embed.conv.weight'] = fs['encoder.pos_conv.0.weight']
    if 'encoder.pos_conv.0.bias' in fs:
        t['encoder.transformer.pos_conv_embed.conv.bias'] = fs['encoder.pos_conv.0.bias']

    # ── Transformer layer norm ────────────────────────────────────────────────
    for suffix in ('weight', 'bias'):
        v = fs.get(f'encoder.layer_norm.{suffix}')
        if v is not None:
            t[f'encoder.transformer.layer_norm.{suffix}'] = v

    # ── Transformer layers ────────────────────────────────────────────────────
    for n in range(12):
        fs_p = f'encoder.layers.{n}'
        ta_p = f'encoder.transformer.layers.{n}'
        for proj in ('k_proj', 'v_proj', 'q_proj'):
            for suffix in ('weight', 'bias'):
                v = fs.get(f'{fs_p}.self_attn.{proj}.{suffix}')
                if v is not None:
                    t[f'{ta_p}.attention.{proj}.{suffix}'] = v
        for suffix in ('weight', 'bias'):
            v = fs.get(f'{fs_p}.self_attn.out_proj.{suffix}')
            if v is not None:
                t[f'{ta_p}.attention.out_proj.{suffix}'] = v
        for suffix in ('weight', 'bias'):
            v = fs.get(f'{fs_p}.self_attn_layer_norm.{suffix}')
            if v is not None:
                t[f'{ta_p}.layer_norm.{suffix}'] = v
        for fc, dense in (('fc1', 'intermediate_dense'), ('fc2', 'output_dense')):
            for suffix in ('weight', 'bias'):
                v = fs.get(f'{fs_p}.{fc}.{suffix}')
                if v is not None:
                    t[f'{ta_p}.feed_forward.{dense}.{suffix}'] = v
        for suffix in ('weight', 'bias'):
            v = fs.get(f'{fs_p}.final_layer_norm.{suffix}')
            if v is not None:
                t[f'{ta_p}.final_layer_norm.{suffix}'] = v

    return t


def _remap_hf_to_torchaudio(hf: dict) -> dict:
    """Remap HuggingFace transformers HubertModel keys to torchaudio Wav2Vec2 layout.

    HF layout differs from torchaudio by two namespacing differences:
      feature_projection.*          → encoder.feature_projection.*
      encoder.{pos_conv,layer_norm,layers}.*  → encoder.transformer.{…}.*
    The feature_extractor CNN keys are identical.
    """
    t: dict = {}
    for k, v in hf.items():
        if k.startswith('feature_extractor.'):
            t[k] = v
        elif k.startswith('feature_projection.'):
            t['encoder.' + k] = v
        elif k.startswith('encoder.pos_conv_embed.'):
            t['encoder.transformer.' + k[len('encoder.'):]] = v
        elif k.startswith('encoder.layer_norm.'):
            t['encoder.transformer.' + k[len('encoder.'):]] = v
        elif k.startswith('encoder.layers.'):
            t['encoder.transformer.' + k[len('encoder.'):]] = v
        # other keys (masked_spec_embed, lm_head, …) are unused — skip
    return t


def _remap_contentvec(raw: dict) -> dict:
    """Auto-detect ContentVec state dict format (fairseq or HF transformers) and remap."""
    # Some checkpoints wrap everything under a model-name prefix
    if raw and all(k.startswith('hubert.') for k in raw):
        raw = {k[len('hubert.'):]: v for k, v in raw.items()}
    # Fairseq checkpoints have this key; HF transformers checkpoints do not
    if 'post_extract_proj.weight' in raw:
        return _remap_fairseq_to_torchaudio(raw)
    return _remap_hf_to_torchaudio(raw)


def _vtln_warp(audio: np.ndarray, alpha: float) -> np.ndarray:
    """Frequency-domain VTLN warp applied to the HuBERT input only.

    Multiplies the apparent frequency of all spectral content by 1/alpha
    without changing array length or sample rate, so pipeline timing is
    unaffected.  alpha < 1 shifts formants upward (male→female); alpha > 1
    shifts them downward.  Only the content features seen by HuBERT are
    affected — F0 is extracted from the original, unwarped audio.
    """
    if abs(alpha - 1.0) < 0.001:
        return audio
    n = len(audio)
    spec = np.fft.rfft(audio)
    n_bins = len(spec)
    k = np.arange(n_bins, dtype=np.float64)
    # Map warped bin k to source bin k*alpha.  alpha<1 → source bin < k
    # → each output frequency draws from a lower (=slower) source frequency
    # → the apparent frequency of every spectral feature is shifted to k/alpha.
    k_src = np.clip(k * alpha, 0.0, n_bins - 1.0)
    lo = k_src.astype(int)
    frac = k_src - lo
    hi = np.minimum(lo + 1, n_bins - 1)
    warped_spec = spec[lo] * (1.0 - frac) + spec[hi] * frac
    return np.fft.irfft(warped_spec, n=n).astype(np.float32)


def _mix_rms(source: np.ndarray, target: np.ndarray, rate: float) -> np.ndarray:
    """Scale `target`'s volume envelope toward `source`'s (WebUI 'rms_mix_rate').

    rate=1 keeps the model's own envelope; rate=0 makes the output follow the
    input dynamics exactly.  Unlike the WebUI we transfer only the envelope
    *shape* (both envelopes are normalised to unit mean first): the input here
    is already RMS-normalised, and absolute-level transfer would tie output
    loudness to mic gain.  Sample rates may differ — envelopes are per-frame
    and interpolated, so only relative timing matters.
    """
    if rate >= 0.999 or len(source) == 0 or len(target) == 0:
        return target
    n_frames = 32   # ~20ms frames over a 640ms window
    def _env(x: np.ndarray) -> np.ndarray:
        frame = max(1, len(x) // n_frames)
        usable = (len(x) // frame) * frame
        e = np.sqrt(np.mean(x[:usable].reshape(-1, frame) ** 2, axis=1))
        e = np.maximum(e, 1e-6)
        return e / e.mean()
    env_s = _env(source)
    env_t = _env(target)
    if len(env_s) != len(env_t):
        env_s = np.interp(np.linspace(0, 1, len(env_t)),
                          np.linspace(0, 1, len(env_s)), env_s)
    gain = (env_s / env_t) ** (1.0 - rate)
    gain = np.clip(gain, 0.0, 4.0)
    gain_full = np.interp(np.linspace(0, 1, len(target)),
                          np.linspace(0, 1, len(gain)), gain)
    return (target * gain_full).astype(np.float32)


def _resample_audio(audio: np.ndarray, orig_sr: int, target_sr: int) -> np.ndarray:
    if orig_sr == target_sr:
        return audio
    import torchaudio.functional as TAF
    t = torch.from_numpy(audio).float().unsqueeze(0)
    t = TAF.resample(t, orig_sr, target_sr)
    return t.squeeze(0).numpy().astype(np.float32)


def _parse_sr(val: object) -> int:
    if isinstance(val, int):
        return val
    s = str(val).lower().strip()
    if s.endswith('k'):
        return int(float(s[:-1]) * 1000)
    return int(s)


def _resize1d(arr: np.ndarray, target: int) -> np.ndarray:
    if len(arr) == target:
        return arr
    src = torch.from_numpy(arr).float().unsqueeze(0).unsqueeze(0)
    dst = F.interpolate(src, size=target, mode='linear', align_corners=True)
    return dst.squeeze().numpy()

