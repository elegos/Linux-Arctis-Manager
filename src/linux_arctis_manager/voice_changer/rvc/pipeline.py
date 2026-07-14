from __future__ import annotations

import logging
import math
import wave
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F

logger = logging.getLogger('RVCPipeline')

# ── Debug recorder ─────────────────────────────────────────────────────────────
# Writes WAV files of the last ~10 s at each pipeline stage so you can open
# them in Audacity to hear exactly what happens at each step.
# Files land in ~/arctis_rvc_debug/ and are overwritten on each save cycle.

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

# Crossfade overlap between adjacent hop slices.
# Set to 0 to disable entirely: consecutive NSF synthesis runs for the same
# time window may have different phases (different transformer attention
# position), so blending them creates phase-cancellation beats that are more
# audible than a clean hard cut at the hop boundary.
_XFADE_OUT = 0

# VAD gate: skip synthesis when new_chunk energy is below this threshold.
# NC-filtered silence is typically 0.001–0.003 RMS; quiet speech is ~0.008+.
# Without this gate, the gain-capped normalizer amplifies residual noise 4×
# and the model synthesises it as voice-like artifacts.
_VAD_RMS = 0.005


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
        self._vtln_alpha: float = 1.0
        self._context_buf: np.ndarray = np.empty(0, dtype=np.float32)  # previous window tail
        self._new_buf: np.ndarray = np.empty(0, dtype=np.float32)      # accumulates until HOP
        self._out_buf: np.ndarray = np.empty(0, dtype=np.float32)
        # Debug rolling buffers: name → (list[chunk], total_samples, sample_rate)
        self._dbg_bufs:  dict[str, list[np.ndarray]] = {}
        self._dbg_total: dict[str, int] = {}
        self._dbg_sr:    dict[str, int] = {}
        self._dbg_cycle: int = 0
        _DEBUG_DIR.mkdir(parents=True, exist_ok=True)
        logger.info('RVC debug buffers enabled — WAV files → %s', _DEBUG_DIR)

    # ── Public ────────────────────────────────────────────────────────────

    def load(self, path: Path, device: torch.device,
             hubert_model: str = 'torchaudio', vtln_alpha: float = 1.0) -> None:
        self._device = device
        self._vtln_alpha = float(vtln_alpha)
        if hubert_model == 'contentvec':
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
        self._load_rmvpe(device)
        logger.info('RVC pipeline ready on %s (model_sr=%d hubert=%s vtln=%.2f)',
                    device, self._model_sr, hubert_model, self._vtln_alpha)

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
        self._context_buf = np.empty(0, dtype=np.float32)
        self._new_buf = np.empty(0, dtype=np.float32)
        self._out_buf = np.zeros(_XFADE_OUT, dtype=np.float32)

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

        # Need current hop + one look-ahead hop so _run_inference gets real future
        # audio as right context instead of an artificial reflect/zero pad.
        # Extra cost: +_HOP_FRAMES of latency (128ms).
        while len(self._new_buf) >= 2 * _HOP_FRAMES:
            new_chunk = self._new_buf[:_HOP_FRAMES]
            look_ahead = self._new_buf[_HOP_FRAMES : 2 * _HOP_FRAMES]
            self._new_buf = self._new_buf[_HOP_FRAMES:]

            # VAD gate: silence → zeros, no inference.  Keeps context current
            # so speech onset has proper history when gate reopens.
            if float(np.sqrt(np.mean(new_chunk ** 2))) < _VAD_RMS:
                n_hop_out = _HOP_FRAMES * _OUTPUT_SR // sr
                self._out_buf = np.concatenate([
                    self._out_buf, np.zeros(n_hop_out, dtype=np.float32)
                ])
                self._context_buf = np.concatenate([self._context_buf, new_chunk])
                if len(self._context_buf) > _CONTEXT_FRAMES:
                    self._context_buf = self._context_buf[-_CONTEXT_FRAMES:]
                continue

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

            # Take XFADE extra samples from the context region (better quality)
            # so the crossfade blend uses context output, not new-chunk output.
            n_take = n_hop_out + _XFADE_OUT
            if len(full_out) >= n_take:
                hop_ext = full_out[-n_take:]
            else:
                hop_ext = np.pad(full_out, (0, n_take - len(full_out)))

            # Per-hop soft limiter: linear below _SOFT_THR, tanh-compressed above.
            # 0.80 catches true clipping peaks without compressing the bulk of
            # the dynamic range — models with higher native output level were
            # audibly distorted by the previous 0.55 threshold.
            _SOFT_THR = 0.80
            _OUT_CEIL = 1.0
            hop_ext = np.where(
                np.abs(hop_ext) <= _SOFT_THR,
                hop_ext,
                np.sign(hop_ext) * (
                    _SOFT_THR + (_OUT_CEIL - _SOFT_THR) *
                    np.tanh((np.abs(hop_ext) - _SOFT_THR) / (_OUT_CEIL - _SOFT_THR))
                ),
            ).astype(np.float32)

            # Stage 5: the new-chunk portion sent to pacat (48 kHz)
            self._dbg_push('05_hop_output_48k', hop_ext[_XFADE_OUT:], _OUTPUT_SR)

            # Retroactive crossfade: blend hop_ext[:XFADE] into the existing
            # tail of _out_buf (fade out old tail, fade in new start), then
            # append the remaining n_hop_out samples.  Output rate unchanged.
            if len(self._out_buf) >= _XFADE_OUT:
                fade_out = np.linspace(1.0, 0.0, _XFADE_OUT, dtype=np.float32)
                fade_in  = 1.0 - fade_out
                self._out_buf[-_XFADE_OUT:] = (
                    self._out_buf[-_XFADE_OUT:] * fade_out +
                    hop_ext[:_XFADE_OUT]        * fade_in
                )
            self._out_buf = np.concatenate([self._out_buf, hop_ext[_XFADE_OUT:]])

            # Slide context forward by one hop
            self._context_buf = np.concatenate([self._context_buf, new_chunk])
            if len(self._context_buf) > _CONTEXT_FRAMES:
                self._context_buf = self._context_buf[-_CONTEXT_FRAMES:]

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
        logger.info('Checkpoint version=%r  f0=%r  sr=%r  info=%r',
                    ckpt.get('version'), ckpt.get('f0'), ckpt.get('sr'),
                    ckpt.get('info', ckpt.get('epoch_info', '—')))
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
            f0 = self._rmvpe.infer(audio)
        else:
            hop = sr // _FEATURE_RATE   # 160 at 16kHz → 100fps
            f0 = _extract_f0_autocorr(audio, sr, hop)
        if pitch_offset != 0.0:
            voiced = f0 > 0
            f0[voiced] *= 2.0 ** (pitch_offset / 12.0)
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
        rms = float(np.sqrt(np.mean(audio ** 2)))
        _TARGET_RMS = 0.10
        if rms > 1e-4:
            norm_gain = min(_TARGET_RMS / rms, 4.0)
            audio = audio * norm_gain
        else:
            norm_gain = 1.0
        peak = float(np.abs(audio).max())
        if peak > 0.90:
            norm_gain = norm_gain * (0.90 / peak)
            audio = audio * (0.90 / peak)
        audio = audio.astype(np.float32)

        # Right context: use real look-ahead audio when available (caller buffers
        # one extra hop before invoking inference).  Real future audio is in-
        # distribution for HuBERT (trained on full bidirectional context); a
        # reflected copy creates an unnatural mirror phoneme sequence that can
        # degrade features at the new-chunk boundary where we extract the hop.
        # Fall back to zero-pad (silence) when no look-ahead is provided — silence
        # is also in-distribution (standard batch-training padding), unlike reflect.
        # Apply the same DC+gain so the concatenated signal is level-consistent.
        if look_ahead is not None and len(look_ahead) == _HOP_FRAMES:
            la = look_ahead.astype(np.float32)
            la = (la - la.mean()) * norm_gain
            right_pad = la
        else:
            right_pad = np.zeros(_HOP_FRAMES, dtype=np.float32)
        audio_padded = np.concatenate([audio, right_pad])  # 10240 samples

        # Stage 3: normalized original window portion only (for audible diagnosis)
        self._dbg_push('03_normalized_16k', audio, sr)

        # VTLN: warp audio in frequency domain before HuBERT so formants shift
        # toward the model's training distribution (alpha<1 = upward shift for
        # male→female).  F0 is extracted from the original, unwarped audio so
        # pitch is unaffected.
        hubert_input = _vtln_warp(audio_padded, self._vtln_alpha)
        feats = self._extract_features(hubert_input)              # [T_f, 768]
        f0, f0_coarse = self._extract_f0(audio_padded, sr, pitch_offset)

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

