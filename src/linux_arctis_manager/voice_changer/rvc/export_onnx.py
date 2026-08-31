"""
One-shot .pth -> ONNX export for an RVC v2 synthesizer checkpoint.

This is "the one Python piece that stays" (see docs/voice-changing-feature.md
and docs/v3-backlog.md's [E10-S6a]/[E10-S7]): a standalone offline
conversion tool invoked once per voice model, not a daemon runtime
dependency. Its only real dependency is `torch` (CPU-only — no
`torchaudio`, no GPU build needed; export/tracing is not latency-sensitive).

`SynthesizerTrnMs768NSFsid.infer()` draws three random tensors internally
(the VITS prior-noise reparameterisation, and the NSF source module's sine
excitation phase + noise), which don't trace to a fixed ONNX graph the way
arithmetic does. `ExportableSynth` below reimplements `.infer()` +
`GeneratorNSF.forward` + `SourceModuleHnNSF.forward` inline, calling the
exact same trained submodules, but takes those three tensors as explicit
`forward()` inputs instead. Before ever exporting, this script proves the
wrapper is bit-exact with the original `.infer()` by capturing the real
random draws from one real call (via a temporary monkeypatch of
`torch.randn_like`/`torch.rand`) and feeding those same tensors to both —
see `_verify_wrapper_bit_exact`.

Usage:
    python -m linux_arctis_manager.voice_changer.rvc.export_onnx <model.pth>

Writes <model>.onnx next to the input file, and numerically verifies the
exported ONNX Runtime output against the PyTorch wrapper's output before
reporting success.
"""
from __future__ import annotations

import argparse
import math
import sys
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F

from linux_arctis_manager.voice_changer.rvc.synth import SynthesizerTrnMs768NSFsid

# Matches the daemon's fixed sliding-window constants (WINDOW_FRAMES=8192 at
# 16kHz -> 26 ContentVec frames at 50fps -> 52 at 100fps after doubling).
T_FEAT = 52


class ExportableSynth(torch.nn.Module):
    """`SynthesizerTrnMs768NSFsid.infer()`, `GeneratorNSF.forward`, and
    `SourceModuleHnNSF.forward` inlined into one deterministic module: the
    three tensors those methods otherwise draw internally are explicit
    forward() parameters instead."""

    def __init__(self, synth: SynthesizerTrnMs768NSFsid) -> None:
        super().__init__()
        self.enc_p = synth.enc_p
        self.flow = synth.flow
        self.dec = synth.dec
        self.emb_g = synth.emb_g
        self._upsample_total = synth._upsample_total

    def forward(
        self,
        phone: torch.Tensor,
        phone_lengths: torch.Tensor,
        pitch: torch.Tensor,
        pitchf: torch.Tensor,
        sid: torch.Tensor,
        prior_noise: torch.Tensor,
        rand_phase: torch.Tensor,
        source_noise: torch.Tensor,
    ) -> torch.Tensor:
        g = self.emb_g(sid).unsqueeze(-1)
        x, m_p, logs_p = self.enc_p(phone, pitch, phone_lengths)
        x_mask = torch.ones(1, 1, x.shape[2], device=x.device, dtype=x.dtype)

        z_p = m_p + prior_noise * torch.exp(logs_p) * 0.33
        z = self.flow(z_p, x_mask, g=g, reverse=True)

        t_audio = z.shape[2] * self._upsample_total
        f0_audio = F.interpolate(
            pitchf.float().unsqueeze(1), size=t_audio, mode='linear', align_corners=True
        ).squeeze(1)

        return self._generator_forward(z * x_mask, f0_audio, g, rand_phase, source_noise)

    def _generator_forward(self, x, f0_audio, g, rand_phase, source_noise):
        dec = self.dec
        har = self._source_module_forward(f0_audio, rand_phase, source_noise).transpose(1, 2)
        x = dec.conv_pre(x)
        if g is not None:
            x = x + dec.cond(g)
        for i, up in enumerate(dec.ups):
            x = F.leaky_relu(x, 0.1)
            x = up(x)
            x = x + dec.noise_convs[i](har)
            xs = sum(dec.resblocks[i * dec.num_kernels + j](x) for j in range(dec.num_kernels))
            x = xs / dec.num_kernels
        x = F.leaky_relu(x)
        return torch.tanh(dec.conv_post(x))

    def _source_module_forward(self, f0, rand_phase, source_noise):
        m = self.dec.m_source
        f0 = f0.unsqueeze(-1)
        uv = (f0 > 1.0).to(f0.dtype)
        rad = 2.0 * math.pi * f0 / m.sample_rate
        phase = torch.cumsum(rad, dim=1)
        sine = (torch.sin(phase + rand_phase) * m.sine_amp) * uv
        noise = m.noise_std * source_noise
        src = sine + noise
        return m.l_tanh(m.l_linear(src.to(m.l_linear.weight.dtype)))


def load_synth(path: Path) -> tuple[SynthesizerTrnMs768NSFsid, list]:
    ckpt = torch.load(path, map_location='cpu', weights_only=False)
    version = ckpt.get('version', 'v1')
    if version != 'v2':
        raise ValueError(f'{path.name}: only RVC v2 (768-dim) models are supported, got {version!r}')
    config = ckpt['config']
    model = SynthesizerTrnMs768NSFsid(*config)
    model.load_state_dict(ckpt['weight'], strict=False)  # enc_q is training-only, expected missing
    model.eval()
    return model, config


def _sample_inputs() -> tuple[torch.Tensor, ...]:
    rng = np.random.RandomState(0)
    phone = torch.from_numpy((rng.randn(1, T_FEAT, 768) * 0.5).astype(np.float32))
    phone_lengths = torch.LongTensor([T_FEAT])
    pitch = torch.from_numpy(rng.randint(1, 255, (1, T_FEAT)).astype(np.int64))
    pitchf = torch.from_numpy(
        np.clip(150.0 + 20.0 * rng.randn(1, T_FEAT), 50, 500).astype(np.float32)
    )
    sid = torch.LongTensor([0])
    return phone, phone_lengths, pitch, pitchf, sid


def _capture_real_draws(synth: SynthesizerTrnMs768NSFsid, sample_inputs) -> tuple[torch.Tensor, ...]:
    """Run the real `.infer()` once, capturing the exact tensors it draws
    internally via a temporary monkeypatch. Returns fresh (non-inference-mode)
    copies of (prior_noise, rand_phase, source_noise), plus the reference
    output for the bit-exactness check."""
    captured: list[tuple[str, torch.Tensor]] = []
    real_randn_like, real_rand = torch.randn_like, torch.rand

    def capture_randn_like(x, *a, **kw):
        t = real_randn_like(x, *a, **kw)
        captured.append(('randn_like', t.clone()))
        return t

    def capture_rand(*a, **kw):
        t = real_rand(*a, **kw)
        captured.append(('rand', t.clone()))
        return t

    torch.randn_like, torch.rand = capture_randn_like, capture_rand
    try:
        with torch.inference_mode():
            ref_out = synth.infer(*sample_inputs)
    finally:
        torch.randn_like, torch.rand = real_randn_like, real_rand

    if len(captured) != 3:
        raise RuntimeError(
            f'expected exactly 3 random draws from .infer(), got {len(captured)} — '
            'synth.py may have changed; ExportableSynth needs updating to match'
        )
    # A tensor created inside torch.inference_mode() stays flagged as an
    # "inference tensor" even after .clone(); the ONNX tracer (which runs
    # under normal autograd) refuses to consume those. A numpy round-trip
    # forces a genuinely fresh, ordinary tensor.
    prior_noise = torch.from_numpy(captured[0][1].numpy())
    rand_phase = torch.from_numpy(captured[1][1].numpy())
    source_noise = torch.from_numpy(captured[2][1].numpy())
    return prior_noise, rand_phase, source_noise, ref_out


def export(model_path: Path) -> Path:
    torch.manual_seed(0)
    synth, _config = load_synth(model_path)
    sample_inputs = _sample_inputs()

    prior_noise, rand_phase, source_noise, ref_out = _capture_real_draws(synth, sample_inputs)

    wrapper = ExportableSynth(synth).eval()
    with torch.inference_mode():
        wrapper_out = wrapper(*sample_inputs, prior_noise, rand_phase, source_noise)
    max_diff = (ref_out - wrapper_out).abs().max().item()
    if max_diff != 0.0:
        raise RuntimeError(
            f'ExportableSynth is not bit-exact with the original .infer() '
            f'(max abs diff {max_diff:.3e}) — refusing to export a possibly-wrong graph'
        )

    onnx_path = model_path.with_suffix('.onnx')
    torch.onnx.export(
        wrapper,
        (*sample_inputs, prior_noise, rand_phase, source_noise),
        str(onnx_path),
        input_names=[
            'phone', 'phone_lengths', 'pitch', 'pitchf', 'sid',
            'prior_noise', 'rand_phase', 'source_noise',
        ],
        output_names=['audio'],
        opset_version=17,
        dynamo=False,
    )

    _verify_onnx_output(onnx_path, sample_inputs, prior_noise, rand_phase, source_noise, wrapper_out)
    return onnx_path


def _verify_onnx_output(onnx_path, sample_inputs, prior_noise, rand_phase, source_noise, expected) -> None:
    import onnxruntime as ort

    phone, phone_lengths, pitch, pitchf, sid = sample_inputs
    feed = {
        'phone': phone.numpy(), 'phone_lengths': phone_lengths.numpy(),
        'pitch': pitch.numpy(), 'pitchf': pitchf.numpy(), 'sid': sid.numpy(),
        'prior_noise': prior_noise.numpy(), 'rand_phase': rand_phase.numpy(),
        'source_noise': source_noise.numpy(),
    }
    sess = ort.InferenceSession(str(onnx_path), providers=['CPUExecutionProvider'])
    actual_inputs = {i.name for i in sess.get_inputs()}
    # Static shapes let the exporter prove some inputs are dead code (e.g.
    # phone_lengths, always == T_FEAT here) and drop them from the graph.
    feed = {k: v for k, v in feed.items() if k in actual_inputs}
    onnx_out = sess.run(None, feed)[0]

    diff = np.abs(onnx_out - expected.detach().numpy())
    print(
        f'ONNX vs PyTorch: max abs diff={diff.max():.3e} mean abs diff={diff.mean():.3e} '
        f'output shape={onnx_out.shape}'
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model_path', type=Path, help='Path to an RVC v2 .pth checkpoint')
    args = parser.parse_args()

    if not args.model_path.is_file():
        print(f'error: {args.model_path} not found', file=sys.stderr)
        raise SystemExit(1)

    onnx_path = export(args.model_path)
    print(f'Exported and verified: {onnx_path}')


if __name__ == '__main__':
    main()
