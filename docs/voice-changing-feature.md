# Voice Changer Feature

## Overview

The Voice Changer adds real-time microphone processing on top of the existing audio pipeline. It exposes a virtual PulseAudio source (`Arctis_VC_Mic`) that any application can select as its microphone input. Two independent modes are offered:

- **LADSPA Effects** — lightweight plugin chain (pitch shift, formant, chorus, delay, distortion, reverb). No GPU required, minimal CPU overhead.
- **AI Voice Changer (RVC)** — neural voice conversion using community `.pth` models. Requires a compatible GPU.

Both modes integrate with the **Microphone** panel in the GUI, which also hosts Noise Cancellation under a second tab and a shared sidetone preview toggle.

---

## Analysis

### Problem Statement

SteelSeries GG on Windows ships a voice changer. Linux users have none. The options evaluated were:

| Approach | CPU | GPU | Latency | Quality |
|---|---|---|---|---|
| LADSPA plugin chain | Low | None | < 5 ms | Good for effects |
| RVC (Retrieval-based Voice Conversion) | Medium | Required | 20–100 ms | Convincing voice clone |
| ONNX Runtime | Medium | Optional | 30–150 ms | Portable, no PyTorch |

**LADSPA** was chosen as the always-available baseline. **RVC** was added as the AI tier because it is the dominant community standard for open-source voice conversion, with thousands of models on HuggingFace.

### Why Not weights.gg?

weights.gg (the previous go-to model repository) was acquired by OpenAI in early 2026 and shut down. HuggingFace Hub is the only large-scale public repository for RVC models at this point.

### Virtual Source Naming

All virtual devices are created with stable `source_name` / `source_properties` so they appear with friendly names in PipeWire / PulseAudio clients:

| PulseAudio name | Description shown to user |
|---|---|
| `Arctis_VC_Sink` | Arctis VC Output (internal sink) |
| `Arctis_VC_Mic` | Arctis Manager Voice Mic |

---

## File Scaffolding

```
src/linux_arctis_manager/
├── voice_changer/
│   ├── __init__.py
│   ├── base.py                  # VoiceChangerBackend ABC + virtual device names
│   ├── settings.py              # VoiceChangerSettings (persisted to YAML)
│   ├── ladspa/
│   │   ├── __init__.py
│   │   ├── effects.py           # LADSPAEffect ABC + 6 concrete effect classes
│   │   └── chain.py             # LADSPAVoiceChanger (VoiceChangerBackend impl)
│   └── rvc/
│       ├── __init__.py
│       ├── backend.py           # RVCBackend ABC
│       ├── pytorch_impl.py      # PyTorchRVCBackend (NVIDIA CUDA / AMD ROCm)
│       ├── openvino_impl.py     # OpenVINORVCBackend (Intel GPU / NPU)
│       ├── registry.py          # BackendRegistry (detects best available backend)
│       ├── model_manager.py     # RVCModelManager + RVCModel + HFModelCard
│       └── rvc_chain.py         # RVCVoiceChanger (VoiceChangerBackend impl)
└── gui/
    ├── vc_widget.py             # QVCWidget + QHFSearchDialog
    └── mic_widget.py            # QMicWidget (hosts NC + VC tabs + sidetone preview)

src/linux_arctis_manager/constants.py
    DBUS_VC_INTERFACE_NAME, DBUS_VC_OBJECT_PATH

src/linux_arctis_manager/core.py
    vc_manager field + reapply_vc()

src/linux_arctis_manager/dbus_service.py
    ArctisManagerDbusVCService

src/linux_arctis_manager/gui/dbus_wrapper.py
    request_vc_capabilities(), request_vc_settings(), set_vc_settings()

src/linux_arctis_manager/lang/en.ini
    [ui] vc_* keys (53 entries)
```

### Installation optional extras (`pyproject.toml`)

```toml
[project.optional-dependencies]
nvidia = ["torch", "huggingface-hub"]
amd    = ["torch", "huggingface-hub"]
intel  = ["openvino", "huggingface-hub"]
rvc    = ["huggingface-hub"]   # model search only, no inference
```

`torch` is not on PyPI — install with the matching index URL:

```bash
# NVIDIA
pipx install "linux-arctis-manager[nvidia]" \
  --pip-args="--extra-index-url https://download.pytorch.org/whl/cu124"

# AMD ROCm
pipx install "linux-arctis-manager[amd]" \
  --pip-args="--extra-index-url https://download.pytorch.org/whl/rocm6.2"

# Intel GPU / NPU
pipx install "linux-arctis-manager[intel]"
```

---

## Architecture

### Class Hierarchy

```
VoiceChangerBackend (ABC)           base.py
├── LADSPAVoiceChanger              ladspa/chain.py
└── RVCVoiceChanger                 rvc/rvc_chain.py

LADSPAEffect (ABC)                  ladspa/effects.py
├── PitchShiftEffect
├── FormantShiftEffect
├── ChorusEffect
├── DelayEffect
├── DistortionEffect
└── ReverbEffect

RVCBackend (ABC)                    rvc/backend.py
├── PyTorchRVCBackend               rvc/pytorch_impl.py
└── OpenVINORVCBackend              rvc/openvino_impl.py
```

### Signal Flow

**LADSPA mode:**
```
Physical mic source
  → module-ladspa-source (PitchShift)
  → module-ladspa-source (Formant)
  → module-ladspa-source (Chorus)
  → module-ladspa-source (Delay)
  → module-ladspa-source (Distortion)
  → module-ladspa-source (Reverb)
  → module-loopback  ──→  Arctis_VC_Sink (null sink)
                              └─ Arctis_VC_Mic (virtual source)
```

Only enabled effects are chained; disabled ones are skipped entirely (no passthrough).

**RVC mode:**
```
Physical mic source
  ─ captured by sounddevice / pulsectl (TODO) ─→
  RVCBackend.process_chunk()  (GPU inference)
  ─ written to ─→ Arctis_VC_Sink (null sink)
                      └─ Arctis_VC_Mic (virtual source)
```

---

## LADSPA Effects Path

### `ladspa/effects.py`

Each effect is a class that:

1. Implements `is_available()` — checks whether its `.so` plugin exists in `LADSPA_PATH` or standard system paths.
2. Returns `install_hint` — per-distro package install commands.
3. Implements `module_args(master, sink_name)` — produces the full argument string for `module-ladspa-source`.

| Class | Plugin | LADSPA ID | Package |
|---|---|---|---|
| `PitchShiftEffect` | `pitch_scale_1193` | 1193 | `ladspa-swh-plugins` |
| `FormantShiftEffect` | `autotalent` | — | `autotalent` |
| `ChorusEffect` | `chorus_1423` | 1423 | `ladspa-swh-plugins` |
| `DelayEffect` | `delay_1898` | 1898 | `ladspa-swh-plugins` |
| `DistortionEffect` | `amp_1181` / `diode_1185` | 1181/1185 | `ladspa-swh-plugins` |
| `ReverbEffect` | `gverb_1216` | 1216 | `ladspa-swh-plugins` |

Plugin discovery uses a candidate list pattern to handle distributions that add numeric suffixes to filenames (e.g. `gate_1410.so` on Fedora).

### `ladspa/chain.py` — `LADSPAVoiceChanger`

`apply(source)`:
1. Tears down any previously loaded chain.
2. Iterates the enabled effect list, calling `module_args()` for each and loading `module-ladspa-source`.
3. Each stage's `source_name` becomes the next stage's `master` — forming a source chain.
4. Loads a `module-null-sink` named `Arctis_VC_Sink` (creating `Arctis_VC_Mic` as its monitor).
5. Loads a `module-loopback` from the last stage to `Arctis_VC_Sink` at 1 ms latency.

`teardown()` unloads all modules in reverse order.

---

## RVC / AI Voice Changer Path

### GPU Backend Detection — `rvc/registry.py`

`BackendRegistry.detect()` iterates the priority list `[PyTorchRVCBackend, OpenVINORVCBackend]` and returns the first whose `is_available()` returns `True`. The result is logged at INFO level.

```python
BackendRegistry._BACKENDS = [PyTorchRVCBackend, OpenVINORVCBackend]
```

### `rvc/pytorch_impl.py` — NVIDIA (CUDA) / AMD (ROCm)

Detection: `torch.cuda.is_available()` — ROCm exposes itself through the CUDA compatibility layer, so a single check covers both vendors.

Vendor is distinguished at label time via `torch.version.hip` (set on ROCm, absent on CUDA).

Model conversion is not required for PyTorch — `.pth` files are loaded natively.

**Current status:** `load_model`, `unload_model`, `process_chunk` are stubs marked `# TODO`.

### `rvc/openvino_impl.py` — Intel GPU / NPU

Detection: `openvino.Core().available_devices` is queried for `'GPU'` or `'NPU'`.

Intel models require a one-time conversion from `.pth` → ONNX → OpenVINO IR:

```
<model>.pth
  → <model>.onnx          (torch.onnx.export, TODO)
  → <model>_openvino_ir/
       model.xml           (openvino.convert_model, TODO)
       model.bin
```

The converted path is cached next to the source `.pth`. `is_model_conversion_required()` checks for `model.xml` existence. The GUI shows a progress bar and "Convert Model" button when conversion is needed.

**Current status:** ONNX export and OpenVINO conversion are stubs. Progress callbacks fire at 0%, 50%, 100% for UI testing.

### `rvc/rvc_chain.py` — `RVCVoiceChanger`

`apply(source)`:
1. Calls `backend.load_model(model_path)`.
2. Creates `Arctis_VC_Sink` null sink (same as LADSPA path).
3. Starts a daemon thread (`arctis-rvc-loop`) for real-time capture → inference → playback.

`teardown()`:
1. Sets `_stop_event`, joins the thread (3 s timeout).
2. Calls `backend.unload_model()`.
3. Unloads PulseAudio modules.

`_process_loop` is a stub. Full implementation requires:
1. Open capture stream on `source` via `sounddevice` or `pulsectl`.
2. Read chunks in a loop while `not _stop_event.is_set()`.
3. Call `self._backend.process_chunk(chunk, sample_rate)`.
4. Write output to `Arctis_VC_Sink`.

---

## Model Management — `rvc/model_manager.py`

### Local Models

Models live in `~/.config/arctis_manager/rvc_models/`. Each model is a `.pth` file, optionally paired with a `.index` file of the same stem (used by some RVC forks for feature retrieval).

```python
@dataclass
class RVCModel:
    name: str
    path: Path
    index_path: Path | None
```

`RVCModelManager.list_local()` scans recursively for `.pth` files.

`RVCModelManager.delete_local(model)` removes the `.pth`, the `.index` (if present), and any `<stem>_openvino_ir/` sibling directory.

### HuggingFace Hub Search

`RVCModelManager.search_hf(query, sort, limit)` uses `huggingface_hub.HfApi.list_models()` with filter `'rvc'`. Sort is `'likes'` (default) or `'downloads'`, configurable per user.

Results are `HFModelCard` dataclasses:

```python
@dataclass
class HFModelCard:
    model_id:   str
    author:     str
    model_name: str
    likes:      int
    downloads:  int
```

`RVCModelManager.download(model_id, progress_cb)` uses `snapshot_download()` to pull the full repo into `MODELS_DIR/<repo-name>/`, then returns the path to the first `.pth` found. Binary files (`.bin`) are excluded from the download.

`search_hf` degrades gracefully when `huggingface_hub` is not installed (returns `[]` with a warning log), so the base package runs without the optional extras.

---

## Settings Persistence — `voice_changer/settings.py`

`VoiceChangerSettings` is saved to `~/.config/arctis_manager/vc_settings.yaml`. All fields have typed defaults; `load()` handles missing keys gracefully.

```yaml
mode: ladspa
pitch:
  enabled: true
  semitones: -3.0
formant:
  enabled: false
  shift: 1.0
chorus: {enabled: false, depth: 2.0, rate: 1.5, delay: 25.0}
delay:  {enabled: false, time: 0.3, feedback: 0.3, damping: 0.5}
distortion: {enabled: false, drive: 6.0}
reverb: {enabled: false, room: 75.0, time: 2.0, wet: 0.3}
rvc:
  model_path: ''
  pitch_shift: 0
  index_ratio: 0.75
  hf_sort: likes
```

Helper methods `to_ladspa_config()` and `to_rvc_config()` return sub-dicts for passing to the respective backend constructors.

---

## GUI

### `gui/mic_widget.py` — `QMicWidget`

Top-level panel widget registered as the `'mic'` nav entry. Contains:

- **Title** label ("Microphone")
- **Sidetone preview toggle** (`QToggle`, default off) — loads a `module-loopback` at 5 ms latency. Source priority: `Arctis_VC_Mic` → `Arctis_NC_Mic` → system default.
- **`QTabWidget`** with two tabs:
  - `Noise Cancellation` → `QNCWidget(show_title=False)`
  - `Voice Changer` → `QVCWidget(show_title=False)`

`hideEvent` auto-stops the sidetone preview when the panel is switched or the window is closed.

`_SidetonePreview` manages the loopback in background threads (start and stop) to avoid blocking the UI thread.

### `gui/vc_widget.py` — `QVCWidget`

Mode selector: **Off / LADSPA Effects / AI Voice Changer**.

**LADSPA section** — one `QGroupBox` per effect. Each group shows:
- Enable toggle
- Per-parameter sliders
- Install hint + Retry button when the plugin is unavailable

**RVC section:**
- Hardware label (from `BackendRegistry.available_label()`)
- Model dropdown (`RVCModelManager.list_local()`)
- "Search HuggingFace..." button → `QHFSearchDialog`
- "Open Folder" button → opens `MODELS_DIR` in the file manager
- Pitch shift slider (semitones)
- Index ratio slider
- Conversion progress bar + "Convert Model" button (shown only for OpenVINO backend)

### `QHFSearchDialog`

Modal dialog with a search field, sort toggle (Likes / Downloads), results table (`QTableWidget`), and a per-row Download button. Download progress is shown inline. The search runs in a background `QThread` to keep the UI responsive.

---

## D-Bus Interface

| Constant | Value |
|---|---|
| `DBUS_VC_INTERFACE_NAME` | `name.giacomofurlan.ArctisManager.Next.VC` |
| `DBUS_VC_OBJECT_PATH` | `/name/giacomofurlan/ArctisManager/Next/VC` |

Methods exposed by `ArctisManagerDbusVCService`:

| Method | Signature | Description |
|---|---|---|
| `GetVCCapabilities` | `→ s (JSON)` | Returns available backends and installed effects |
| `GetVCSettings` | `→ s (JSON)` | Returns current `VoiceChangerSettings` as JSON |
| `SetVCSettings` | `s (JSON) →` | Applies new settings, re-chains if mode is active |

GUI side (in `DbusWrapper`): `request_vc_capabilities()`, `request_vc_settings()`, `set_vc_settings()`.

---

## Virtual Device Names

| Constant | PulseAudio name | Displayed as |
|---|---|---|
| `ARCTIS_VC_SINK` | `Arctis_VC_Sink` | Arctis VC Output |
| `ARCTIS_VC_MIC` | `Arctis_VC_Mic` | Arctis Manager Voice Mic |
| `ARCTIS_VC_MIC_DESC` | — | `'Arctis Manager Voice Mic'` |

The NC mic (`Arctis_NC_Mic`) sits upstream of the VC mic in the sidetone preview priority chain, so the most-processed available source is always selected.

---

## Current Implementation Status

| Component | Status |
|---|---|
| LADSPA effect chain | Complete |
| PulseAudio module loading / teardown | Complete |
| Virtual source creation | Complete |
| Sidetone preview loopback | Complete |
| Settings persistence | Complete |
| HuggingFace model search | Complete |
| HuggingFace model download | Complete |
| Local model management | Complete |
| GPU backend detection (PyTorch) | Complete |
| GPU backend detection (OpenVINO) | Complete |
| RVC model loading — PyTorch | **TODO** |
| RVC model loading — OpenVINO | **TODO** |
| RVC inference (`process_chunk`) — PyTorch | **TODO** |
| RVC inference (`process_chunk`) — OpenVINO | **TODO** |
| ONNX export for OpenVINO conversion | **TODO** |
| Real-time capture → inference → playback loop | **TODO** |

The TODO items are confined to `rvc/pytorch_impl.py`, `rvc/openvino_impl.py`, and `_process_loop` in `rvc/rvc_chain.py`. All scaffolding, PulseAudio plumbing, GUI, settings, and model management are fully implemented.
