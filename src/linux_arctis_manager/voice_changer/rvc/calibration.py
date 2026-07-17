"""Guided voice calibration for the RVC pipeline.

Flow (driven by the GUI wizard over D-Bus):
  1. The user reads a short edge-case text (phrase endings, nasals, plosives,
     quiet trail-offs) while `record_start`/`record_stop` capture the raw mic.
  2. `propose_variants` builds three labeled parameter candidates around the
     model's current tuning; `render` streams the recording through a fresh
     pipeline instance per candidate — same 128 ms hop cadence as the live
     chain, so artifacts heard here are the artifacts the live chain makes.
  3. The user listens (original + three renders) and picks by ear; objective
     metrics have repeatedly disagreed with perception, so the ear decides.
     "Refine" runs another round with narrower steps around the pick.

Renders run on the daemon's GPU alongside the live chain; a calibration is
expected to happen outside calls, so the occasional latency hiccup during
rendering is acceptable.
"""
from __future__ import annotations

import logging
import subprocess
import threading
import wave
from dataclasses import asdict
from pathlib import Path

import numpy as np

from linux_arctis_manager.voice_changer.rvc.backend import RVCParams

logger = logging.getLogger('RVCCalibration')

CALIBRATION_DIR = Path.home() / '.cache' / 'arctis_manager' / 'calibration'

_RECORD_SR = 16000
_HOP = 2048                    # 128 ms — must match the live chain cadence
_MAX_RECORD_SECS = 120


def propose_variants(base: RVCParams, refine_around: RVCParams | None = None,
                     ) -> list[tuple[str, RVCParams]]:
    """Three labeled candidates: the current tuning plus two contrasts.

    First round: A = current, B = dynamics-faithful (output follows the
    input envelope more closely, softer drive), C = model-forward (let the
    model's own envelope and formants through).  Refine round: half-steps
    around the previously chosen candidate on the two most audible axes.
    """
    if refine_around is not None:
        x = refine_around
        lo = RVCParams(**{**asdict(x),
                          'target_rms':   max(0.04, x.target_rms * 0.85),
                          'rms_mix_rate': max(0.0, x.rms_mix_rate - 0.15)})
        hi = RVCParams(**{**asdict(x),
                          'target_rms':   min(0.20, x.target_rms * 1.15),
                          'rms_mix_rate': min(1.0, x.rms_mix_rate + 0.15)})
        return [('A', x), ('B', lo), ('C', hi)]

    b = base
    faithful = RVCParams(**{**asdict(b),
                            'rms_mix_rate': max(0.0, b.rms_mix_rate - 0.35),
                            'target_rms':   max(0.04, b.target_rms * 0.7),
                            'limiter_thr':  min(b.limiter_thr, 0.80)})
    forward = RVCParams(**{**asdict(b),
                           'rms_mix_rate': min(1.0, b.rms_mix_rate + 0.25),
                           'target_rms':   min(0.20, b.target_rms * 1.3),
                           'vtln_alpha':   1.0 if b.vtln_alpha != 1.0 else 0.88,
                           'limiter_thr':  1.0})
    return [('A', b), ('B', faithful), ('C', forward)]


class CalibrationSession:
    """One recording + render cycle.  Owned by the D-Bus VC service."""

    def __init__(self) -> None:
        self._rec_proc: subprocess.Popen | None = None
        self._rec_buf: bytearray = bytearray()
        self._rec_thread: threading.Thread | None = None
        self._render_thread: threading.Thread | None = None
        self._lock = threading.Lock()
        self.state: str = 'idle'    # idle|recording|recorded|rendering|done|error
        self.error: str = ''
        self.recording_path: Path | None = None
        self.results: list[dict] = []   # [{'label', 'params', 'path'}]
        self.peak: float = 0.0          # recording peak (0..1), silence detector

    # ── Recording ─────────────────────────────────────────────────────

    def record_start(self, source_id: str) -> bool:
        with self._lock:
            if self.state == 'recording':
                return False
            CALIBRATION_DIR.mkdir(parents=True, exist_ok=True)
            # pw-record, not parec: the NC/VC sources are native PipeWire
            # filter-chain nodes that the PulseAudio compat layer does not
            # always expose — parec silently records nothing from them.
            # '-' streams raw samples to stdout.
            # Stereo float32, downmixed by averaging in record_stop: asking
            # pw-record for mono makes it SUM the source channels (measured
            # 2-3× gain), hard-clipping speech — clipped input corrupts
            # HuBERT features and the renders come out as static-like noise.
            cmd = ['pw-record', '--target', source_id,
                   '--rate', str(_RECORD_SR), '--channels', '2',
                   '--format', 'f32', '-']
            try:
                self._rec_proc = subprocess.Popen(
                    cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            except Exception as e:
                self.state, self.error = 'error', f'parec failed: {e}'
                logger.error('calibration record_start: %s', e)
                return False
            self._rec_buf = bytearray()
            self._rec_thread = threading.Thread(
                target=self._drain_rec, daemon=True, name='vc-calib-rec')
            self.state, self.error = 'recording', ''
            self._rec_thread.start()
            logger.info('calibration recording started from %r', source_id)
            return True

    def _drain_rec(self) -> None:
        proc = self._rec_proc
        assert proc is not None and proc.stdout is not None
        limit = _MAX_RECORD_SECS * _RECORD_SR * 8   # f32 stereo
        while True:
            chunk = proc.stdout.read(4096)
            if not chunk:
                break
            self._rec_buf.extend(chunk)
            if len(self._rec_buf) >= limit:
                proc.terminate()
                break

    def record_stop(self) -> str:
        with self._lock:
            if self.state != 'recording':
                return ''
            proc = self._rec_proc
            if proc is not None:
                try:
                    proc.terminate()
                    proc.wait(timeout=2)
                except Exception:
                    proc.kill()
            if self._rec_thread is not None:
                self._rec_thread.join(timeout=2)
            self._rec_proc = None

            path = CALIBRATION_DIR / 'original.wav'
            # Raw f32 stereo → average-downmix to mono, then s16 wav.
            buf = bytes(self._rec_buf)
            buf = buf[:len(buf) // 8 * 8]
            frames = np.frombuffer(buf, dtype=np.float32).reshape(-1, 2)
            frames = np.nan_to_num(frames, nan=0.0, posinf=0.0, neginf=0.0)
            mono = np.clip(frames.mean(axis=1), -1.0, 1.0)
            with wave.open(str(path), 'w') as f:
                f.setnchannels(1)
                f.setsampwidth(2)
                f.setframerate(_RECORD_SR)
                f.writeframes((mono * 32767).astype(np.int16).tobytes())
            secs = len(mono) / _RECORD_SR
            self.peak = float(np.abs(mono).max()) if mono.size else 0.0
            self.recording_path = path
            self.state = 'recorded'
            logger.info('calibration recording stopped: %.1f s, peak %.3f → %s',
                        secs, self.peak, path)
            return str(path)

    # ── Rendering ─────────────────────────────────────────────────────

    def render_start(self, model_path: Path, pitch_offset: float,
                     variants: list[tuple[str, RVCParams]]) -> bool:
        with self._lock:
            if self.state not in ('recorded', 'done', 'error') or self.recording_path is None:
                return False
            self.state, self.error, self.results = 'rendering', '', []
            self._render_thread = threading.Thread(
                target=self._render, daemon=True, name='vc-calib-render',
                args=(model_path, pitch_offset, list(variants)))
            self._render_thread.start()
            return True

    def _render(self, model_path: Path, pitch_offset: float,
                variants: list[tuple[str, RVCParams]]) -> None:
        try:
            import torch

            from linux_arctis_manager.voice_changer.rvc.pipeline import RVCPipeline

            assert self.recording_path is not None
            with wave.open(str(self.recording_path)) as f:
                sig = np.frombuffer(f.readframes(f.getnframes()),
                                    dtype=np.int16).astype(np.float32) / 32768

            device = torch.device('cuda' if torch.cuda.is_available() else 'cpu')
            results: list[dict] = []
            for label, params in variants:
                pipe = RVCPipeline()
                pipe.load(model_path, device, params)
                out = []
                for i in range(0, len(sig) - _HOP + 1, _HOP):
                    out.append(pipe.convert(sig[i:i + _HOP], _RECORD_SR, pitch_offset))
                for _ in range(8):   # flush pipeline latency
                    out.append(pipe.convert(
                        np.zeros(_HOP, dtype=np.float32), _RECORD_SR, pitch_offset))
                pipe.unload()
                o = np.concatenate(out)
                path = CALIBRATION_DIR / f'variant_{label.lower()}.wav'
                with wave.open(str(path), 'w') as f:
                    f.setnchannels(1)
                    f.setsampwidth(2)
                    f.setframerate(48000)
                    f.writeframes((np.clip(o, -1, 1) * 32767).astype(np.int16).tobytes())
                results.append({'label': label,
                                'params': asdict(params),
                                'path': str(path)})
                logger.info('calibration variant %s rendered → %s', label, path)
            self.results = results
            self.state = 'done'
        except Exception as e:
            logger.exception('calibration render failed')
            self.state, self.error = 'error', str(e)

    # ── Status ────────────────────────────────────────────────────────

    def status(self) -> dict:
        return {
            'state': self.state,
            'error': self.error,
            'recording': str(self.recording_path) if self.recording_path else '',
            'results': self.results,
            'peak': self.peak,
        }
