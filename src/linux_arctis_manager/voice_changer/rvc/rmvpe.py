from __future__ import annotations

import logging
from pathlib import Path
from typing import List

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

logger = logging.getLogger('RMVPE')



# ── Mel spectrogram (torchaudio, no librosa/scipy) ────────────────────────────

class _MelSpectrogram(nn.Module):
    def __init__(self) -> None:
        super().__init__()
        import torchaudio
        mel = torchaudio.functional.melscale_fbanks(
            n_freqs=513, f_min=30.0, f_max=8000.0,
            n_mels=128, sample_rate=16000,
            norm=None, mel_scale='htk',
        ).T  # [128, 513]
        self.register_buffer('mel_basis', mel.float())
        self.register_buffer('window', torch.hann_window(1024))

    def forward(self, audio: torch.Tensor) -> torch.Tensor:
        fft = torch.stft(audio, n_fft=1024, hop_length=160, win_length=1024,
                         window=self.window, center=True, return_complex=True)
        mag = torch.abs(fft)                           # [B, 513, T]
        mel = torch.matmul(self.mel_basis, mag)        # [B, 128, T]
        return torch.log(torch.clamp(mel, min=1e-5))


# ── RMVPE network (E2E = DeepUnet + BiGRU → 360 salience bins) ───────────────
# Attribute names match the reference checkpoint exactly so state_dict loads cleanly.

class _BiGRU(nn.Module):
    def __init__(self, input_features: int, hidden_features: int, num_layers: int):
        super().__init__()
        self.gru = nn.GRU(input_features, hidden_features, num_layers=num_layers,
                          batch_first=True, bidirectional=True)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.gru(x)[0]


class _ConvBlockRes(nn.Module):
    def __init__(self, in_channels: int, out_channels: int, momentum: float = 0.01):
        super().__init__()
        self.conv = nn.Sequential(
            nn.Conv2d(in_channels, out_channels, (3, 3), (1, 1), (1, 1), bias=False),
            nn.BatchNorm2d(out_channels, momentum=momentum), nn.ReLU(),
            nn.Conv2d(out_channels, out_channels, (3, 3), (1, 1), (1, 1), bias=False),
            nn.BatchNorm2d(out_channels, momentum=momentum), nn.ReLU(),
        )
        if in_channels != out_channels:
            self.shortcut = nn.Conv2d(in_channels, out_channels, (1, 1))

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        if hasattr(self, 'shortcut'):
            return self.conv(x) + self.shortcut(x)
        return self.conv(x) + x


class _ResEncoderBlock(nn.Module):
    def __init__(self, in_channels: int, out_channels: int,
                 kernel_size, n_blocks: int = 1, momentum: float = 0.01):
        super().__init__()
        self.n_blocks = n_blocks
        self.conv = nn.ModuleList(
            [_ConvBlockRes(in_channels, out_channels, momentum)] +
            [_ConvBlockRes(out_channels, out_channels, momentum) for _ in range(n_blocks - 1)]
        )
        self.kernel_size = kernel_size
        if kernel_size is not None:
            self.pool = nn.AvgPool2d(kernel_size=kernel_size)

    def forward(self, x: torch.Tensor):
        for c in self.conv:
            x = c(x)
        if self.kernel_size is not None:
            return x, self.pool(x)
        return x


class _Encoder(nn.Module):
    def __init__(self, in_channels: int, in_size: int, n_encoders: int,
                 kernel_size, n_blocks: int, out_channels: int = 16,
                 momentum: float = 0.01):
        super().__init__()
        self.n_encoders = n_encoders
        self.bn = nn.BatchNorm2d(in_channels, momentum=momentum)
        self.layers = nn.ModuleList()
        self.latent_channels: list = []
        for _ in range(n_encoders):
            self.layers.append(_ResEncoderBlock(in_channels, out_channels,
                                                kernel_size, n_blocks, momentum))
            self.latent_channels.append([out_channels, in_size])
            in_channels = out_channels
            out_channels *= 2
            in_size //= 2
        self.out_size = in_size
        self.out_channel = out_channels   # doubled after last iter (= 512 for 5 layers, start 16)

    def forward(self, x: torch.Tensor):
        concat_tensors: List[torch.Tensor] = []
        x = self.bn(x)
        for layer in self.layers:
            t, x = layer(x)
            concat_tensors.append(t)
        return x, concat_tensors


class _Intermediate(nn.Module):
    def __init__(self, in_channels: int, out_channels: int,
                 n_inters: int, n_blocks: int, momentum: float = 0.01):
        super().__init__()
        self.n_inters = n_inters
        self.layers = nn.ModuleList(
            [_ResEncoderBlock(in_channels, out_channels, None, n_blocks, momentum)] +
            [_ResEncoderBlock(out_channels, out_channels, None, n_blocks, momentum)
             for _ in range(n_inters - 1)]
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        for layer in self.layers:
            x = layer(x)
        return x


class _ResDecoderBlock(nn.Module):
    def __init__(self, in_channels: int, out_channels: int,
                 stride, n_blocks: int = 1, momentum: float = 0.01):
        super().__init__()
        out_padding = (0, 1) if stride == (1, 2) else (1, 1)
        self.n_blocks = n_blocks
        self.conv1 = nn.Sequential(
            nn.ConvTranspose2d(in_channels, out_channels, (3, 3), stride=stride,
                               padding=(1, 1), output_padding=out_padding, bias=False),
            nn.BatchNorm2d(out_channels, momentum=momentum),
            nn.ReLU(),
        )
        self.conv2 = nn.ModuleList(
            [_ConvBlockRes(out_channels * 2, out_channels, momentum)] +
            [_ConvBlockRes(out_channels, out_channels, momentum) for _ in range(n_blocks - 1)]
        )

    def forward(self, x: torch.Tensor, concat_tensor: torch.Tensor) -> torch.Tensor:
        x = self.conv1(x)
        x = torch.cat([x, concat_tensor], dim=1)
        for c in self.conv2:
            x = c(x)
        return x


class _Decoder(nn.Module):
    def __init__(self, in_channels: int, n_decoders: int,
                 stride, n_blocks: int, momentum: float = 0.01):
        super().__init__()
        self.n_decoders = n_decoders
        self.layers = nn.ModuleList()
        for _ in range(n_decoders):
            out_channels = in_channels // 2
            self.layers.append(_ResDecoderBlock(in_channels, out_channels,
                                                stride, n_blocks, momentum))
            in_channels = out_channels

    def forward(self, x: torch.Tensor,
                concat_tensors: List[torch.Tensor]) -> torch.Tensor:
        for i, layer in enumerate(self.layers):
            x = layer(x, concat_tensors[-1 - i])
        return x


class _DeepUnet(nn.Module):
    def __init__(self, kernel_size, n_blocks: int, en_de_layers: int = 5,
                 inter_layers: int = 4, in_channels: int = 1,
                 en_out_channels: int = 16):
        super().__init__()
        self.encoder = _Encoder(in_channels, 128, en_de_layers, kernel_size,
                                n_blocks, en_out_channels)
        self.intermediate = _Intermediate(
            self.encoder.out_channel // 2,   # actual last-layer output channels
            self.encoder.out_channel,
            inter_layers, n_blocks,
        )
        self.decoder = _Decoder(self.encoder.out_channel, en_de_layers,
                                kernel_size, n_blocks)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x, skips = self.encoder(x)
        x = self.intermediate(x)
        return self.decoder(x, skips)


class _E2E(nn.Module):
    def __init__(self, n_blocks: int, n_gru: int, kernel_size,
                 en_de_layers: int = 5, inter_layers: int = 4,
                 in_channels: int = 1, en_out_channels: int = 16):
        super().__init__()
        self.unet = _DeepUnet(kernel_size, n_blocks, en_de_layers, inter_layers,
                              in_channels, en_out_channels)
        self.cnn = nn.Conv2d(en_out_channels, 3, (3, 3), padding=(1, 1))
        self.fc = nn.Sequential(
            _BiGRU(3 * 128, 256, n_gru),
            nn.Linear(512, 360),
            nn.Dropout(0.25),
            nn.Sigmoid(),
        )

    def forward(self, mel: torch.Tensor) -> torch.Tensor:
        # mel: [B, 128, T] → [B, 1, T, 128] for 2D conv
        x = mel.transpose(-1, -2).unsqueeze(1)
        x = self.cnn(self.unet(x))        # [B, 3, T, 128]
        x = x.transpose(1, 2).flatten(-2) # [B, T, 384]
        return self.fc(x)                  # [B, T, 360]


# ── Load ─────────────────────────────────────────────────────────────────────

def ensure_model() -> Path:
    from linux_arctis_manager.voice_changer.rvc.model_downloader import RMVPE, model_path
    path = model_path(RMVPE)
    if path is None:
        raise FileNotFoundError(
            'RMVPE model not found. '
            'Download it from the Voice Changer → Base Models section.'
        )
    return path


class RMVPE:
    """Neural F0 estimator at 100 fps, purpose-built for RVC voice conversion."""

    def __init__(self, device: torch.device) -> None:
        path = ensure_model()
        self.device = device
        model = _E2E(4, 1, (2, 2))
        ckpt = torch.load(path, map_location='cpu', weights_only=False)
        model.load_state_dict(ckpt)
        model.eval().to(device)
        self._model = model
        self._mel = _MelSpectrogram().to(device)
        cents = 20 * np.arange(360) + 1997.3794084376191
        self._cents = np.pad(cents, (4, 4))  # [368]
        logger.info('RMVPE loaded on %s', device)

    @torch.inference_mode()
    def infer(self, audio: np.ndarray, threshold: float = 0.03) -> tuple[np.ndarray, np.ndarray]:
        """audio: float32 at 16 kHz.

        Returns (f0, confidence): F0 Hz at 100 fps (0 = unvoiced) and the
        per-frame salience peak (0..1) — weak for creak/fry, strong for
        cleanly-phonated (including sung) frames.
        """
        wav = torch.from_numpy(audio).float().unsqueeze(0).to(self.device)
        mel = self._mel(wav)                   # [1, 128, T]
        n_frames = mel.shape[-1]
        n_pad = 32 * ((n_frames - 1) // 32 + 1) - n_frames
        if n_pad > 0:
            mel = F.pad(mel, (0, n_pad))
        hidden = self._model(mel)[:, :n_frames].squeeze(0).cpu().numpy()  # [T, 360]
        return self._decode(hidden, threshold)

    def _decode(self, salience: np.ndarray, thred: float) -> tuple[np.ndarray, np.ndarray]:
        center  = np.argmax(salience, axis=1) + 4          # [T], offset into padded
        sal_pad = np.pad(salience, ((0, 0), (4, 4)))       # [T, 368]
        sal_win = np.stack([sal_pad[i, c - 4:c + 5] for i, c in enumerate(center)])
        map_win = np.stack([self._cents[c - 4:c + 5]  for c in center])
        cents   = np.sum(sal_win * map_win, 1) / (np.sum(sal_win, 1) + 1e-10)
        peak = np.max(salience, axis=1)
        cents[peak <= thred] = 0
        f0 = 10.0 * (2.0 ** (cents / 1200))
        f0[f0 == 10] = 0
        # Onset backfill: at a soft phrase start the first frames of a vowel
        # have weak salience and are marked unvoiced — the NSF vocoder then
        # synthesises noise excitation there (garbled starts).  Extend voicing
        # BACKWARD only, into weak frames (≥ thred/3) directly preceding a
        # confident onset, copying the onset's F0 (weak frames' own estimates
        # are too noisy to trust).  Backward-only + copied F0 keeps the
        # decision stable across overlapping hops; symmetric hysteresis made
        # marginal frames flip voiced/unvoiced between hops → crackle.
        weak = peak > (thred / 3.0)
        onsets = np.flatnonzero((f0[1:] > 0) & (f0[:-1] == 0)) + 1
        for i in onsets:
            j = i - 1
            while j >= 0 and i - j <= 5 and weak[j] and f0[j] == 0:
                f0[j] = f0[i]
                j -= 1
        return f0.astype(np.float32), peak.astype(np.float32)
