from __future__ import annotations

import math

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.nn import Conv1d, ConvTranspose1d
from torch.nn.utils import weight_norm, remove_weight_norm


def _get_padding(kernel_size: int, dilation: int = 1) -> int:
    return (kernel_size * dilation - dilation) // 2


class LayerNorm(nn.Module):
    def __init__(self, channels: int, eps: float = 1e-5) -> None:
        super().__init__()
        self.eps = eps
        self.gamma = nn.Parameter(torch.ones(channels))
        self.beta = nn.Parameter(torch.zeros(channels))

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = x.transpose(1, -1)
        x = F.layer_norm(x, x.shape[-1:], self.gamma, self.beta, self.eps)
        return x.transpose(1, -1)


class MultiHeadAttention(nn.Module):
    def __init__(self, channels: int, out_channels: int, n_heads: int,
                 window_size: int | None = None, p_dropout: float = 0.0) -> None:
        super().__init__()
        assert channels % n_heads == 0
        self.n_heads = n_heads
        self.k_channels = channels // n_heads
        self.window_size = window_size
        self.conv_q = Conv1d(channels, channels, 1)
        self.conv_k = Conv1d(channels, channels, 1)
        self.conv_v = Conv1d(channels, channels, 1)
        self.conv_o = Conv1d(channels, out_channels, 1)
        self.drop = nn.Dropout(p_dropout)
        if window_size is not None:
            self.emb_rel_k = nn.Parameter(torch.zeros(1, window_size * 2 + 1, self.k_channels))
            self.emb_rel_v = nn.Parameter(torch.zeros(1, window_size * 2 + 1, self.k_channels))

    def forward(self, x: torch.Tensor, attn_mask: torch.Tensor | None = None) -> torch.Tensor:
        q, k, v = self.conv_q(x), self.conv_k(x), self.conv_v(x)
        out = self._attention(q, k, v, attn_mask)
        return self.conv_o(out)

    def _get_rel_emb(self, emb: torch.Tensor, length: int) -> torch.Tensor:
        w = self.window_size  # type: ignore[arg-type]
        pad = max(length - (w + 1), 0)
        start = max((w + 1) - length, 0)
        end = start + 2 * length - 1
        if pad > 0:
            emb = F.pad(emb, [0, 0, pad, pad, 0, 0])
        return emb[:, start:end]

    @staticmethod
    def _rel2abs(x: torch.Tensor) -> torch.Tensor:
        B, H, T, _ = x.shape
        x = F.pad(x, [0, 1]).view(B, H, -1)
        x = F.pad(x, [0, T - 1]).view(B, H, T + 1, 2 * T - 1)
        return x[:, :, :T, T - 1:]

    @staticmethod
    def _abs2rel(x: torch.Tensor) -> torch.Tensor:
        B, H, T, _ = x.shape
        x = F.pad(x, [0, T - 1]).view(B, H, -1)
        x = F.pad(x, [T, 0]).view(B, H, T, 2 * T)
        return x[:, :, :, 1:]

    def _attention(self, q: torch.Tensor, k: torch.Tensor, v: torch.Tensor,
                   mask: torch.Tensor | None) -> torch.Tensor:
        B, C, T = q.shape
        h, d = self.n_heads, self.k_channels
        q = q.view(B, h, d, T).transpose(2, 3)  # [B,h,T,d]
        k = k.view(B, h, d, T).transpose(2, 3)
        v = v.view(B, h, d, T).transpose(2, 3)

        scale = 1.0 / math.sqrt(d)
        scores = torch.matmul(q * scale, k.transpose(-2, -1))

        if self.window_size is not None:
            rk = self._get_rel_emb(self.emb_rel_k, T)  # [1, 2T-1, d]
            rel_logits = torch.matmul(q * scale, rk.unsqueeze(0).transpose(-2, -1))
            scores = scores + self._rel2abs(rel_logits)

        if mask is not None:
            scores = scores.masked_fill(mask == 0, -1e4)
        w = self.drop(F.softmax(scores, dim=-1))

        out = torch.matmul(w, v)
        if self.window_size is not None:
            rv = self._get_rel_emb(self.emb_rel_v, T)  # [1, 2T-1, d]
            rel_out = torch.matmul(self._abs2rel(w), rv.unsqueeze(0))
            out = out + rel_out

        return out.transpose(2, 3).contiguous().view(B, C, T)


class FFN(nn.Module):
    def __init__(self, in_channels: int, out_channels: int, filter_channels: int,
                 kernel_size: int, p_dropout: float = 0.0) -> None:
        super().__init__()
        self.conv_1 = Conv1d(in_channels, filter_channels, kernel_size,
                             padding=kernel_size // 2)
        self.conv_2 = Conv1d(filter_channels, out_channels, kernel_size,
                             padding=kernel_size // 2)
        self.drop = nn.Dropout(p_dropout)

    def forward(self, x: torch.Tensor, x_mask: torch.Tensor) -> torch.Tensor:
        x = F.relu(self.conv_1(x * x_mask))
        x = self.drop(x)
        return self.conv_2(x * x_mask) * x_mask


class Encoder(nn.Module):
    def __init__(self, hidden_channels: int, filter_channels: int, n_heads: int,
                 n_layers: int, kernel_size: int = 1, p_dropout: float = 0.0,
                 window_size: int = 4) -> None:
        super().__init__()
        self.drop = nn.Dropout(p_dropout)
        self.attn_layers = nn.ModuleList([
            MultiHeadAttention(hidden_channels, hidden_channels, n_heads,
                               window_size=window_size, p_dropout=p_dropout)
            for _ in range(n_layers)
        ])
        self.norm_layers_1 = nn.ModuleList([LayerNorm(hidden_channels) for _ in range(n_layers)])
        self.ffn_layers = nn.ModuleList([
            FFN(hidden_channels, hidden_channels, filter_channels, kernel_size, p_dropout)
            for _ in range(n_layers)
        ])
        self.norm_layers_2 = nn.ModuleList([LayerNorm(hidden_channels) for _ in range(n_layers)])

    def forward(self, x: torch.Tensor, x_mask: torch.Tensor) -> torch.Tensor:
        attn_mask = x_mask.unsqueeze(2) * x_mask.unsqueeze(-1)
        for attn, n1, ffn, n2 in zip(
                self.attn_layers, self.norm_layers_1, self.ffn_layers, self.norm_layers_2):
            y = self.drop(attn(x * x_mask, attn_mask))
            x = n1(x + y)
            y = self.drop(ffn(x, x_mask))
            x = n2(x + y)
        return x * x_mask


class WN(nn.Module):
    def __init__(self, hidden_channels: int, kernel_size: int, dilation_rate: int,
                 n_layers: int, gin_channels: int = 0) -> None:
        super().__init__()
        self.hidden_channels = hidden_channels
        self.n_layers = n_layers
        self.in_layers = nn.ModuleList()
        self.res_skip_layers = nn.ModuleList()
        if gin_channels:
            self.cond_layer = weight_norm(
                Conv1d(gin_channels, 2 * hidden_channels * n_layers, 1))
        for i in range(n_layers):
            dil = dilation_rate ** i
            pad = _get_padding(kernel_size, dil)
            self.in_layers.append(weight_norm(
                Conv1d(hidden_channels, 2 * hidden_channels, kernel_size,
                       dilation=dil, padding=pad)))
            out_ch = 2 * hidden_channels if i < n_layers - 1 else hidden_channels
            self.res_skip_layers.append(weight_norm(Conv1d(hidden_channels, out_ch, 1)))

    def forward(self, x: torch.Tensor, x_mask: torch.Tensor,
                g: torch.Tensor | None = None) -> torch.Tensor:
        out = torch.zeros_like(x)
        g_split: torch.Tensor | None = self.cond_layer(g) if g is not None and hasattr(self, 'cond_layer') else None
        hc = self.hidden_channels
        for i, (il, rsl) in enumerate(zip(self.in_layers, self.res_skip_layers)):
            x_in = il(x)
            if g_split is not None:
                x_in = x_in + g_split[:, 2 * hc * i: 2 * hc * (i + 1)]
            a, b = x_in.chunk(2, dim=1)
            acts = torch.tanh(a) * torch.sigmoid(b)
            rs = rsl(acts)
            if i < self.n_layers - 1:
                res, skip = rs.chunk(2, dim=1)
                x = (x + res) * x_mask
                out = out + skip
            else:
                out = out + rs
        return out * x_mask


class ResidualCouplingLayer(nn.Module):
    def __init__(self, channels: int, hidden_channels: int, kernel_size: int,
                 dilation_rate: int, n_layers: int, gin_channels: int = 0) -> None:
        super().__init__()
        half = channels // 2
        self.half = half
        self.pre = Conv1d(half, hidden_channels, 1)
        self.enc = WN(hidden_channels, kernel_size, dilation_rate, n_layers, gin_channels)
        self.post = Conv1d(hidden_channels, half, 1)
        nn.init.zeros_(self.post.weight)
        nn.init.zeros_(self.post.bias)  # type: ignore[arg-type]

    def forward(self, x: torch.Tensor, x_mask: torch.Tensor,
                g: torch.Tensor | None = None, reverse: bool = False) -> torch.Tensor:
        x0, x1 = x[:, :self.half], x[:, self.half:]
        h = self.enc(self.pre(x0) * x_mask, x_mask, g)
        m = self.post(h) * x_mask
        x1 = (x1 - m) if reverse else (x1 + m)
        return torch.cat([x0, x1], dim=1) * x_mask


class Flip(nn.Module):
    def forward(self, x: torch.Tensor, *args,  # type: ignore[override]
                reverse: bool = False, **kwargs) -> torch.Tensor:
        return x.flip(1)


class ResidualCouplingBlock(nn.Module):
    def __init__(self, channels: int, hidden_channels: int, kernel_size: int,
                 dilation_rate: int, n_layers: int, n_flows: int = 4,
                 gin_channels: int = 0) -> None:
        super().__init__()
        self.flows = nn.ModuleList()
        for _ in range(n_flows):
            self.flows.append(ResidualCouplingLayer(
                channels, hidden_channels, kernel_size, dilation_rate,
                n_layers, gin_channels))
            self.flows.append(Flip())

    def forward(self, x: torch.Tensor, x_mask: torch.Tensor,
                g: torch.Tensor | None = None, reverse: bool = False) -> torch.Tensor:
        itr = reversed(self.flows) if reverse else iter(self.flows)
        for f in itr:
            x = f(x, x_mask, g=g, reverse=reverse)
        return x


class TextEncoder768(nn.Module):
    def __init__(self, out_channels: int, hidden_channels: int, filter_channels: int,
                 n_heads: int, n_layers: int, kernel_size: int,
                 p_dropout: float, f0: bool = True) -> None:
        super().__init__()
        self.out_channels = out_channels
        self.hidden_channels = hidden_channels
        self.f0 = f0
        self.emb_phone = nn.Linear(768, hidden_channels)
        self.lrelu = nn.LeakyReLU(0.1, inplace=True)
        if f0:
            self.emb_pitch = nn.Embedding(256, hidden_channels)
        self.encoder = Encoder(hidden_channels, filter_channels, n_heads,
                               n_layers, kernel_size, p_dropout, window_size=10)
        self.proj = Conv1d(hidden_channels, out_channels * 2, 1)

    def forward(self, phone: torch.Tensor, pitch: torch.Tensor | None,
                lengths: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor]:
        x = self.emb_phone(phone)
        if self.f0 and pitch is not None:
            x = x + self.emb_pitch(pitch)
        x = x * math.sqrt(self.hidden_channels)
        x = self.lrelu(x)
        x = x.transpose(1, 2)   # [B, H, T]
        x_mask = torch.ones(x.shape[0], 1, x.shape[2], device=x.device, dtype=x.dtype)
        x = self.encoder(x * x_mask, x_mask)
        stats = self.proj(x) * x_mask
        m, logs = stats.chunk(2, dim=1)
        return x, m, logs


class ResBlock1(nn.Module):
    def __init__(self, channels: int, kernel_size: int = 3,
                 dilation: tuple = (1, 3, 5)) -> None:
        super().__init__()
        self.convs1 = nn.ModuleList([
            weight_norm(Conv1d(channels, channels, kernel_size, dilation=d,
                               padding=_get_padding(kernel_size, d)))
            for d in dilation
        ])
        self.convs2 = nn.ModuleList([
            weight_norm(Conv1d(channels, channels, kernel_size,
                               padding=_get_padding(kernel_size, 1)))
            for _ in dilation
        ])

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        for c1, c2 in zip(self.convs1, self.convs2):
            xt = F.leaky_relu(x, 0.1)
            xt = c2(F.leaky_relu(c1(xt), 0.1))
            x = x + xt
        return x

    def remove_weight_norm(self) -> None:
        for c in (*self.convs1, *self.convs2):
            remove_weight_norm(c)


class SourceModuleHnNSF(nn.Module):
    def __init__(self, sample_rate: int, harmonic_num: int = 0,
                 sine_amp: float = 0.1, noise_std: float = 0.003) -> None:
        super().__init__()
        self.sine_amp = sine_amp
        self.noise_std = noise_std
        self.sample_rate = sample_rate
        self.l_linear = nn.Linear(harmonic_num + 1, 1)
        self.l_tanh = nn.Tanh()

    def forward(self, f0: torch.Tensor) -> torch.Tensor:
        # f0: [B, T] in Hz at audio sample rate; T = total audio samples
        with torch.no_grad():
            f0 = f0.unsqueeze(-1)                          # [B, T, 1]
            uv = (f0 > 1.0).to(f0.dtype)
            rad = 2.0 * math.pi * f0 / self.sample_rate
            phase = torch.cumsum(rad, dim=1)
            rand_phase = torch.rand(f0.shape[0], 1, 1, device=f0.device, dtype=f0.dtype)
            sine = (torch.sin(phase + rand_phase) * self.sine_amp) * uv
            noise = self.noise_std * torch.randn_like(sine)
            src = sine + noise
        return self.l_tanh(self.l_linear(src.to(self.l_linear.weight.dtype)))


class GeneratorNSF(nn.Module):
    def __init__(self, initial_channel: int,
                 resblock_kernel_sizes: list[int],
                 resblock_dilation_sizes: list[list[int]],
                 upsample_rates: list[int],
                 upsample_initial_channel: int,
                 upsample_kernel_sizes: list[int],
                 gin_channels: int, sr: int) -> None:
        super().__init__()
        self.num_kernels = len(resblock_kernel_sizes)
        self.conv_pre = Conv1d(initial_channel, upsample_initial_channel, 7, padding=3)
        self.ups = nn.ModuleList()
        self.noise_convs = nn.ModuleList()
        channels = [upsample_initial_channel]
        for i, (u, k) in enumerate(zip(upsample_rates, upsample_kernel_sizes)):
            ch = upsample_initial_channel // (2 ** (i + 1))
            channels.append(ch)
            self.ups.append(weight_norm(ConvTranspose1d(
                channels[i], ch, k, stride=u, padding=(k - u) // 2)))
            import math as _math
            stride = _math.prod(upsample_rates[i + 1:]) if i + 1 < len(upsample_rates) else 1
            if stride > 1:
                self.noise_convs.append(Conv1d(1, ch, stride * 2, stride=stride,
                                               padding=stride // 2))
            else:
                self.noise_convs.append(Conv1d(1, ch, 1))
        self.resblocks = nn.ModuleList()
        for i in range(len(self.ups)):
            ch = channels[i + 1]
            for k, d in zip(resblock_kernel_sizes, resblock_dilation_sizes):
                self.resblocks.append(ResBlock1(ch, k, tuple(d)))
        self.conv_post = Conv1d(channels[-1], 1, 7, padding=3)
        if gin_channels:
            self.cond = Conv1d(gin_channels, upsample_initial_channel, 1)
        self.m_source = SourceModuleHnNSF(sr, harmonic_num=0)

    def forward(self, x: torch.Tensor, f0_audio: torch.Tensor,
                g: torch.Tensor | None = None) -> torch.Tensor:
        # x: [B, C, T_feat]; f0_audio: [B, T_audio] at audio sample rate
        har = self.m_source(f0_audio).transpose(1, 2)   # [B, 1, T_audio]
        x = self.conv_pre(x)
        if g is not None:
            x = x + self.cond(g)
        for i, up in enumerate(self.ups):
            x = F.leaky_relu(x, 0.1)
            x = up(x)
            x = x + self.noise_convs[i](har)
            xs = sum(self.resblocks[i * self.num_kernels + j](x)
                     for j in range(self.num_kernels))
            x = xs / self.num_kernels
        x = F.leaky_relu(x)
        return torch.tanh(self.conv_post(x))

    def remove_weight_norm(self) -> None:
        remove_weight_norm(self.conv_pre)
        remove_weight_norm(self.conv_post)
        for up in self.ups:
            remove_weight_norm(up)
        for rb in self.resblocks:
            rb.remove_weight_norm()
