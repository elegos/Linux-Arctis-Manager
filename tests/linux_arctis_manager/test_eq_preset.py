import math

from linux_arctis_manager.eq_preset import (
    BUILTIN_PRESETS,
    MBEQ_BAND_FREQUENCIES,
    SIMPLE_BAND_FREQUENCIES,
    SIMPLE_BAND_INDICES,
    EQBand,
    EQPreset,
    downsample_bands,
    elevate_bands,
    list_presets,
)

# --- EQPreset.__post_init__ / defaults ---

def test_simple_preset_gets_default_bands_at_simple_frequencies():
    preset = EQPreset(name='Test')
    assert [b.frequency for b in preset.bands] == SIMPLE_BAND_FREQUENCIES
    assert all(b.gain == 0.0 for b in preset.bands)


def test_advanced_preset_gets_default_bands_at_all_frequencies():
    preset = EQPreset(name='Test', mode='advanced')
    assert [b.frequency for b in preset.bands] == MBEQ_BAND_FREQUENCIES


def test_preset_keeps_explicit_bands():
    bands = [EQBand(frequency=100, gain=2.0)]
    preset = EQPreset(name='Test', bands=bands)
    assert preset.bands == bands


# --- to_ladspa_controls ---

def test_to_ladspa_controls_simple_mode_maps_onto_15_slots():
    preset = EQPreset(name='Test', mode='simple')
    preset.bands[0].gain = 5.0
    controls = preset.to_ladspa_controls()
    assert len(controls) == 15
    assert controls[SIMPLE_BAND_INDICES[0]] == 5.0
    # Unset slots stay at 0.
    assert controls[4] == 0.0


def test_to_ladspa_controls_advanced_mode_maps_1_to_1():
    preset = EQPreset(name='Test', mode='advanced')
    preset.bands[3].gain = -2.5
    controls = preset.to_ladspa_controls()
    assert controls[3] == -2.5


def test_to_ladspa_controls_simple_mode_ignores_extra_bands():
    preset = EQPreset(name='Test', mode='simple', bands=[EQBand(frequency=f) for f in SIMPLE_BAND_FREQUENCIES])
    controls = preset.to_ladspa_controls()
    assert len(controls) == 15


# --- save / load ---

def test_save_and_load_round_trip(tmp_path, monkeypatch):
    monkeypatch.setattr('linux_arctis_manager.eq_preset.EQ_PRESETS_FOLDER', tmp_path)
    preset = EQPreset(name='My Preset', description='desc')
    preset.bands[0].gain = 3.5

    path = preset.save()
    assert path.exists()
    assert path.name == 'my_preset.yaml'

    loaded = EQPreset.load(path)
    assert loaded.name == 'My Preset'
    assert loaded.description == 'desc'
    assert loaded.mode == 'simple'
    assert loaded.bands[0].gain == 3.5


def test_save_with_explicit_path(tmp_path):
    preset = EQPreset(name='X')
    target = tmp_path / 'custom.yaml'
    result = preset.save(path=target)
    assert result == target
    assert target.exists()


def test_load_defaults_missing_optional_fields(tmp_path):
    target = tmp_path / 'minimal.yaml'
    target.write_text('name: Minimal\nbands: []\n')
    loaded = EQPreset.load(target)
    assert loaded.name == 'Minimal'
    assert loaded.mode == 'simple'
    assert loaded.description == ''


# --- flat ---

def test_flat_returns_all_zero_gains():
    preset = EQPreset.flat()
    assert preset.name == 'Flat'
    assert all(b.gain == 0.0 for b in preset.bands)


# --- BUILTIN_PRESETS ---

def test_builtin_presets_are_flagged_builtin_and_simple_mode():
    assert len(BUILTIN_PRESETS) == 8
    for preset in BUILTIN_PRESETS:
        assert preset.builtin is True
        assert preset.mode == 'simple'
        assert len(preset.bands) == len(SIMPLE_BAND_FREQUENCIES)


# --- elevate_bands / downsample_bands ---

def test_elevate_bands_preserves_known_gains_at_matching_indices():
    bands_10 = [EQBand(frequency=f, gain=float(i)) for i, f in enumerate(SIMPLE_BAND_FREQUENCIES)]
    elevated = elevate_bands(bands_10)
    assert len(elevated) == 15
    for ui_idx, mbeq_idx in enumerate(SIMPLE_BAND_INDICES):
        assert elevated[mbeq_idx].gain == float(ui_idx)


def test_elevate_bands_interpolates_missing_indices_between_neighbors():
    bands_10 = [EQBand(frequency=f, gain=0.0) for f in SIMPLE_BAND_FREQUENCIES]
    # Index 3 -> mbeq_idx 3 (known=0.0); the ones in between are missing.
    bands_10[3] = EQBand(frequency=SIMPLE_BAND_FREQUENCIES[3], gain=10.0)
    elevated = elevate_bands(bands_10)
    missing_idx = 4  # frequency 311, between mbeq indices 3 and 5
    lo_gain = elevated[3].gain
    hi_gain = elevated[5].gain
    assert min(lo_gain, hi_gain) <= elevated[missing_idx].gain <= max(lo_gain, hi_gain)


def test_downsample_bands_keeps_exact_values_at_simple_frequencies():
    bands_15 = [EQBand(frequency=f, gain=0.0) for f in MBEQ_BAND_FREQUENCIES]
    for idx in SIMPLE_BAND_INDICES:
        bands_15[idx].gain = 7.0
    downsampled = downsample_bands(bands_15)
    assert all(b.gain == 7.0 for b in downsampled)


def test_downsample_bands_redistributes_extra_band_gain_to_neighbors():
    bands_15 = [EQBand(frequency=f, gain=0.0) for f in MBEQ_BAND_FREQUENCIES]
    extra_idx = 4  # not in SIMPLE_BAND_INDICES
    bands_15[extra_idx].gain = 10.0
    downsampled = downsample_bands(bands_15)
    total_gain = sum(b.gain for b in downsampled)
    assert math.isclose(total_gain, 10.0, abs_tol=1e-6)


def test_elevate_bands_zero_gain_stays_flat():
    # Not an inverse of downsample_bands (elevate interpolates band values;
    # downsample redistributes energy) — just check the trivial flat case.
    bands_10 = [EQBand(frequency=f, gain=0.0) for f in SIMPLE_BAND_FREQUENCIES]
    elevated = elevate_bands(bands_10)
    assert all(b.gain == 0.0 for b in elevated)


# --- list_presets ---

def test_list_presets_includes_builtins_when_no_folder(tmp_path, monkeypatch):
    monkeypatch.setattr('linux_arctis_manager.eq_preset.EQ_PRESETS_FOLDER', tmp_path / 'nonexistent')
    result = list_presets()
    assert result == BUILTIN_PRESETS


def test_list_presets_includes_saved_custom_presets(tmp_path, monkeypatch):
    monkeypatch.setattr('linux_arctis_manager.eq_preset.EQ_PRESETS_FOLDER', tmp_path)
    custom = EQPreset(name='Custom')
    custom.save()
    result = list_presets()
    names = [p.name for p in result]
    assert 'Custom' in names
    assert len(result) == len(BUILTIN_PRESETS) + 1


def test_list_presets_skips_unparseable_files(tmp_path, monkeypatch):
    monkeypatch.setattr('linux_arctis_manager.eq_preset.EQ_PRESETS_FOLDER', tmp_path)
    (tmp_path / 'broken.yaml').write_text('not: [valid, eq, preset')
    result = list_presets()
    assert result == BUILTIN_PRESETS
