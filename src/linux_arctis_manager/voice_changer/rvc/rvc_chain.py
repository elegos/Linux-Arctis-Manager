from __future__ import annotations

import logging
import queue
import subprocess
import threading

import numpy as np
import pulsectl

from linux_arctis_manager.voice_changer.rvc.backend import RVCBackend
from linux_arctis_manager.voice_changer.rvc.registry import BackendRegistry

logger = logging.getLogger('RVCVoiceChanger')

ARCTIS_VC_SINK     = 'Arctis_VC_Sink'
ARCTIS_VC_MIC      = 'Arctis_VC_Mic'
ARCTIS_VC_MIC_DESC = 'Arctis Manager VC Mic'

_SAMPLE_RATE    = 16000    # Hz — parec capture rate (HuBERT input)
_PLAYBACK_RATE  = 48000    # Hz — pacat/null-sink rate (model native output, no downsample)
_CHUNK_FRAMES   = 1024     # frames per processing chunk at _SAMPLE_RATE
_CHUNK_BYTES    = _CHUNK_FRAMES * 2   # int16 = 2 bytes/frame
_RATE_RATIO     = _PLAYBACK_RATE // _SAMPLE_RATE   # = 3
_FORMAT         = 's16le'
_CHANNELS       = 1


class RVCVoiceChanger:
    """
    Routes microphone audio through an RVC model using:
      parec (PulseAudio record) → convert (GPU inference) → pacat (PulseAudio play)

    Architecture:
      capture_thread  – reads raw PCM from parec subprocess
      convert_thread  – runs RVC inference on queued chunks
      playback_thread – writes processed PCM via pacat subprocess
    """

    def __init__(self) -> None:
        self._backend: RVCBackend | None = None
        self._capture_proc:  subprocess.Popen | None = None
        self._playback_proc: subprocess.Popen | None = None
        self._capture_thread:  threading.Thread | None = None
        self._convert_thread:  threading.Thread | None = None
        self._playback_thread: threading.Thread | None = None
        self._raw_q:       queue.Queue[bytes | None] = queue.Queue(maxsize=32)
        self._processed_q: queue.Queue[bytes | None] = queue.Queue(maxsize=32)
        self._running = False
        self._null_sink_module: int | None = None
        self._pulse: pulsectl.Pulse | None = None

    # ── Public API ────────────────────────────────────────────────────────

    def apply(self, source_id: str, model_name: str, pitch_offset: float,
              hubert_model: str = 'torchaudio', vtln_alpha: float = 1.0) -> bool:
        self.teardown()

        # Recreate queues: teardown's final capture-loop put() may have left a
        # stale None sentinel that would kill the new run's convert thread instantly.
        self._raw_q = queue.Queue(maxsize=32)
        self._processed_q = queue.Queue(maxsize=32)

        backend = BackendRegistry.best_backend()
        if backend is None:
            logger.error('RVC: no GPU backend available')
            return False

        from linux_arctis_manager.voice_changer.rvc.model_manager import RVCModelManager
        model = RVCModelManager.find_model(model_name)
        if model is None:
            logger.error('RVC: model %r not found', model_name)
            return False

        try:
            backend.load_model(model.path, hubert_model=hubert_model, vtln_alpha=vtln_alpha)
        except Exception as e:
            logger.error('RVC: failed to load model %r: %s', model_name, e)
            return False

        self._backend = backend

        if not self._ensure_virtual_devices():
            return False

        self._running = True
        self._capture_proc = self._start_parec(source_id)
        self._playback_proc = self._start_pacat()

        self._capture_thread = threading.Thread(
            target=self._capture_loop, daemon=True, name='rvc-capture')
        self._convert_thread = threading.Thread(
            target=self._convert_loop, daemon=True,
            args=(pitch_offset,), name='rvc-convert')
        self._playback_thread = threading.Thread(
            target=self._playback_loop, daemon=True, name='rvc-playback')

        self._capture_thread.start()
        self._convert_thread.start()
        self._playback_thread.start()

        logger.info('RVC chain started: source=%r model=%r pitch=%.1f hubert=%s vtln=%.2f',
                    source_id, model_name, pitch_offset, hubert_model, vtln_alpha)
        return True

    def teardown(self) -> None:
        self._running = False

        # Drain queues to unblock threads
        for q in (self._raw_q, self._processed_q):
            try:
                q.put_nowait(None)
            except queue.Full:
                pass

        for proc in (self._capture_proc, self._playback_proc):
            if proc and proc.poll() is None:
                try:
                    proc.terminate()
                except Exception:
                    pass

        for t in (self._capture_thread, self._convert_thread, self._playback_thread):
            if t and t.is_alive():
                t.join(timeout=2.0)

        self._capture_proc = self._playback_proc = None
        self._capture_thread = self._convert_thread = self._playback_thread = None

        if self._backend:
            self._backend.unload_model()
            self._backend = None

        self._unload_virtual_devices()
        logger.info('RVC chain torn down')

    # ── PulseAudio virtual devices ────────────────────────────────────────

    def _ensure_virtual_devices(self) -> bool:
        try:
            pulse = self._pulse_conn()
            # Tear down any pre-existing sink: it may be stale from a previous
            # run at a different sample rate (e.g., old 16kHz sink).
            for s in pulse.sink_list():
                if s.name == ARCTIS_VC_SINK or s.proplist.get('node.name', '') == ARCTIS_VC_SINK:
                    try:
                        pulse.module_unload(s.owner_module)
                        logger.debug('RVC: removed stale null sink (module %d)', s.owner_module)
                    except Exception:
                        pass
            # No device.class=filter: KDE propagates that class to the monitor
            # source, hiding it from the input device list.
            idx = pulse.module_load(
                'module-null-sink',
                f'sink_name={ARCTIS_VC_SINK} '
                f'node.description="Arctis VC Output" '
                f'channels={_CHANNELS} rate={_PLAYBACK_RATE}',
            )
            self._null_sink_module = idx
            logger.info('RVC null sink created (module %d) at %d Hz', idx, _PLAYBACK_RATE)
            return True
        except Exception as e:
            logger.error('RVC: failed to create virtual devices: %s', e)
            return False

    def _unload_virtual_devices(self) -> None:
        pulse = self._pulse_conn()
        if self._null_sink_module is not None:
            try:
                pulse.module_unload(self._null_sink_module)
            except Exception as e:
                logger.warning('RVC: failed to unload null sink: %s', e)
        self._null_sink_module = None
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None

    def _pulse_conn(self) -> pulsectl.Pulse:
        if self._pulse is None:
            self._pulse = pulsectl.Pulse('arctis-rvc-manager')
        return self._pulse

    # ── Subprocess helpers ────────────────────────────────────────────────

    def _start_parec(self, source_id: str) -> subprocess.Popen:
        cmd = [
            'parec',
            f'--device={source_id}',
            '--raw',
            f'--channels={_CHANNELS}',
            f'--rate={_SAMPLE_RATE}',
            f'--format={_FORMAT}',
            '--latency-msec=20',
        ]
        logger.debug('RVC parec: %s', ' '.join(cmd))
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        threading.Thread(target=self._log_stderr, args=(proc, 'parec'),
                         daemon=True, name='rvc-parec-stderr').start()
        return proc

    def _start_pacat(self) -> subprocess.Popen:
        cmd = [
            'pacat',
            f'--device={ARCTIS_VC_SINK}',
            '--raw',
            f'--channels={_CHANNELS}',
            f'--rate={_PLAYBACK_RATE}',
            f'--format={_FORMAT}',
            '--latency-msec=200',
        ]
        logger.debug('RVC pacat: %s', ' '.join(cmd))
        proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stderr=subprocess.PIPE)
        threading.Thread(target=self._log_stderr, args=(proc, 'pacat'),
                         daemon=True, name='rvc-pacat-stderr').start()
        return proc

    @staticmethod
    def _log_stderr(proc: subprocess.Popen, name: str) -> None:
        assert proc.stderr is not None
        for line in proc.stderr:
            line = line.decode(errors='replace').rstrip()
            if line:
                logger.warning('RVC %s: %s', name, line)
        rc = proc.wait()
        if rc not in (0, -15):   # -15 = SIGTERM (expected on teardown)
            logger.warning('RVC %s exited with code %d', name, rc)

    # ── Processing threads ────────────────────────────────────────────────

    def _capture_loop(self) -> None:
        proc = self._capture_proc
        if proc is None or proc.stdout is None:
            return
        chunks = 0
        try:
            buf = b''
            while self._running:
                data = proc.stdout.read(_CHUNK_BYTES - len(buf))
                if not data:
                    break
                buf += data
                if len(buf) >= _CHUNK_BYTES:
                    self._raw_q.put(buf[:_CHUNK_BYTES])
                    buf = buf[_CHUNK_BYTES:]
                    chunks += 1
                    if chunks == 1:
                        logger.debug('RVC capture: first chunk received')
        except Exception as e:
            logger.warning('RVC capture loop: %s', e)
        finally:
            logger.debug('RVC capture loop exiting (chunks=%d, running=%s)', chunks, self._running)
            self._raw_q.put(None)

    def _convert_loop(self, pitch_offset: float) -> None:
        backend = self._backend
        chunks = 0
        try:
            while True:
                data = self._raw_q.get()
                if data is None or not self._running:
                    break
                # int16 → float32 [-1, 1]
                samples = np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0
                try:
                    if backend:
                        converted = backend.convert(samples, _SAMPLE_RATE, pitch_offset)
                    else:
                        converted = np.repeat(samples, _RATE_RATIO)
                except Exception as e:
                    logger.warning('RVC convert: %s', e)
                    converted = np.repeat(samples, _RATE_RATIO)
                # float32 → int16
                out = np.clip(converted * 32767.0, -32768, 32767).astype(np.int16)
                self._processed_q.put(out.tobytes())
                chunks += 1
                if chunks == 1:
                    logger.debug('RVC convert: first chunk processed')
        except Exception as e:
            logger.warning('RVC convert loop: %s', e)
        finally:
            logger.debug('RVC convert loop exiting (chunks=%d)', chunks)
            self._processed_q.put(None)

    def _playback_loop(self) -> None:
        proc = self._playback_proc
        if proc is None or proc.stdin is None:
            return
        chunks = 0
        try:
            while True:
                data = self._processed_q.get()
                if data is None or not self._running:
                    break
                proc.stdin.write(data)
                proc.stdin.flush()
                chunks += 1
                if chunks == 1:
                    logger.debug('RVC playback: first chunk written to pacat')
        except Exception as e:
            logger.warning('RVC playback loop: %s', e)
        finally:
            logger.debug('RVC playback loop exiting (chunks=%d)', chunks)
