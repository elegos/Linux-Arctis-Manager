from __future__ import annotations

import math

import torch
import torch.nn as nn
import torch.nn.functional as F

from linux_arctis_manager.voice_changer.rvc.synth_modules import (
    TextEncoder768,
    ResidualCouplingBlock,
    GeneratorNSF,
)


class SynthesizerTrnMs768NSFsid(nn.Module):
    def __init__(self,
                 spec_channels: int,
                 segment_size: int,
                 inter_channels: int,
                 hidden_channels: int,
                 filter_channels: int,
                 n_heads: int,
                 n_layers: int,
                 kernel_size: int,
                 p_dropout: float,
                 resblock: str,
                 resblock_kernel_sizes: list,
                 resblock_dilation_sizes: list,
                 upsample_rates: list,
                 upsample_initial_channel: int,
                 upsample_kernel_sizes: list,
                 n_speakers: int,
                 gin_channels: int,
                 sr: int,
                 **kwargs) -> None:
        super().__init__()
        self.inter_channels = inter_channels
        self.gin_channels = gin_channels
        # Total upsampling factor — must match GeneratorNSF's actual product;
        # hardcoding 400 breaks 48kHz models (total=480).
        self._upsample_total = math.prod(upsample_rates)

        self.enc_p = TextEncoder768(
            inter_channels, hidden_channels, filter_channels,
            n_heads, n_layers, kernel_size, p_dropout, f0=True)

        self.dec = GeneratorNSF(
            inter_channels, resblock_kernel_sizes, resblock_dilation_sizes,
            upsample_rates, upsample_initial_channel, upsample_kernel_sizes,
            gin_channels=gin_channels, sr=sr)

        # Flow: kernel_size=5, dilation_rate=1, n_layers=3 (confirmed from checkpoint)
        self.flow = ResidualCouplingBlock(
            inter_channels, hidden_channels, 5, 1, 3,
            n_flows=4, gin_channels=gin_channels)

        if n_speakers > 0:
            self.emb_g = nn.Embedding(n_speakers, gin_channels)

    @torch.inference_mode()
    def infer(self, phone: torch.Tensor, phone_lengths: torch.Tensor,
              pitch: torch.Tensor, pitchf: torch.Tensor,
              sid: torch.Tensor) -> torch.Tensor:
        """
        phone:        [B, T, 768] HuBERT features (100fps after doubling)
        phone_lengths:[B]         sequence lengths
        pitch:        [B, T]      quantised F0 indices [0..255]
        pitchf:       [B, T]      float F0 in Hz (0 = unvoiced); at feature rate
        sid:          [B]         speaker id (use 0 for single-speaker inference)
        returns:      [B, 1, T_audio] waveform at model sr
        """
        g = self.emb_g(sid).unsqueeze(-1)          # [B, gin_ch, 1]
        x, m_p, logs_p = self.enc_p(phone, pitch, phone_lengths)
        x_mask = torch.ones(1, 1, x.shape[2], device=x.device, dtype=x.dtype)
        z_p = m_p + torch.randn_like(m_p) * torch.exp(logs_p)
        z = self.flow(z_p, x_mask, g=g, reverse=True)

        # Interpolate F0 to audio sample rate using the model's actual upsample total
        T_audio = z.shape[2] * self._upsample_total
        f0_audio = F.interpolate(
            pitchf.float().unsqueeze(1), size=T_audio,
            mode='linear', align_corners=True).squeeze(1)

        return self.dec(z * x_mask, f0_audio, g=g)
