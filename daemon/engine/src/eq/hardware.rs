// Hardware EQ payload encoding for SteelSeries Arctis families.
//
// All functions are pure (no I/O).  The caller feeds the returned bytes to the
// HID transport.  Padding to the HID report size (64 or 65 bytes) is done here
// so callers don't need to know the report geometry.

use super::preset::{BandMode, EqBand, FilterType};

// ── Capability descriptor ─────────────────────────────────────────────────────

/// HID EQ protocol variant for a device family.
///
/// Populated from device YAML config; used to select the right encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwEqFormat {
    /// Nova Pro wired / Nova Pro Wireless / Nova Elite.
    /// 10 bands, fixed frequencies, gain encoded as u8 = (gain_f32 + 10) × 2.
    /// Report ID varies per model (Nova Pro Wireless TX = 0x06; Nova Pro wired = 0x00).
    NovaPro {
        /// HID report ID byte prepended to every payload.
        report_id: u8,
        /// Command byte for setting the 10-band custom EQ (0x33).
        eq_cmd: u8,
        /// Command byte for selecting the active preset (0x2E).
        preset_cmd: u8,
    },
    /// Nova 3 Wireless / Nova 5 / Nova 7 Gen 2 / Nova 7P Gen 2.
    /// 10 parametric bands: (freq u16 LE, filter_type u8, gain_i8 = gain_f32 × 10).
    /// Two connections: RF uses `rf_cmd`, BT uses `bt_cmd` (adjacent cmd byte).
    ParametricNova {
        /// HID report ID byte prepended to every payload.
        report_id: u8,
        /// Command byte for 2.4 GHz connection (0x33).
        rf_cmd: u8,
        /// Command byte for Bluetooth connection (0x34).
        bt_cmd: u8,
    },
}

/// Describes the hardware EQ capability of a connected device.
#[derive(Debug, Clone)]
pub struct HwEqCapability {
    pub format: HwEqFormat,
    /// `BandMode` that this hardware format natively supports.
    pub native_band_mode: BandMode,
}

impl HwEqCapability {
    /// Standard Nova Pro Wireless TX configuration.
    pub fn nova_pro_wireless() -> Self {
        Self {
            format: HwEqFormat::NovaPro {
                report_id: 0x06,
                eq_cmd: 0x33,
                preset_cmd: 0x2E,
            },
            native_band_mode: BandMode::Fixed10,
        }
    }

    /// Standard Nova Pro wired configuration.
    pub fn nova_pro_wired() -> Self {
        Self {
            format: HwEqFormat::NovaPro {
                report_id: 0x00,
                eq_cmd: 0x33,
                preset_cmd: 0x2E,
            },
            native_band_mode: BandMode::Fixed10,
        }
    }

    /// Nova 3 / Nova 5 / Nova 7 Gen 2 configuration.
    pub fn nova_parametric() -> Self {
        Self {
            format: HwEqFormat::ParametricNova {
                report_id: 0x00,
                rf_cmd: 0x33,
                bt_cmd: 0x34,
            },
            native_band_mode: BandMode::Parametric10,
        }
    }
}

// ── NovaPro encoder ───────────────────────────────────────────────────────────

/// Encode a 10-band fixed-frequency EQ payload for the NovaPro family.
///
/// Layout: `[report_id, eq_cmd, gain1..gain10, 0x00 × padding]` padded to 64 bytes.
/// Gain encoding: `u8 = clamp((gain_f32 + 10.0) × 2.0, 0, 40)`.
/// Gains outside ±10 dB are clamped to the device range.
pub fn encode_nova_pro_eq(report_id: u8, eq_cmd: u8, bands: &[EqBand]) -> Vec<u8> {
    let mut buf = vec![0u8; 64];
    buf[0] = report_id;
    buf[1] = eq_cmd;
    for (i, band) in bands.iter().take(10).enumerate() {
        buf[2 + i] = nova_pro_gain_byte(band.gain);
    }
    buf
}

/// Encode the preset-select command to activate the Custom EQ slot (id 4).
///
/// Layout: `[report_id, preset_cmd, 0x04, 0x00 × padding]` padded to 64 bytes.
pub fn encode_nova_pro_preset_select(report_id: u8, preset_cmd: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 64];
    buf[0] = report_id;
    buf[1] = preset_cmd;
    buf[2] = 0x04; // 0x04 = Custom EQ slot
    buf
}

fn nova_pro_gain_byte(gain_f32: f32) -> u8 {
    let raw = (gain_f32 + 10.0) * 2.0;
    raw.clamp(0.0, 40.0).round() as u8
}

// ── ParametricNova encoder ────────────────────────────────────────────────────

/// Filter type byte values in the ParametricNova protocol.
///
/// Values derived from device spec (`filter_type uint8 range 1 6`).
/// Only low-shelf (1), peaking (2), and high-shelf (3) are mapped by the
/// `FilterType` enum; the remaining values are reserved by firmware.
fn filter_type_byte(ft: FilterType) -> u8 {
    match ft {
        FilterType::LowShelf => 1,
        FilterType::Peaking => 2,
        FilterType::HighShelf => 3,
    }
}

fn gain_i8_from_f32(gain: f32) -> i8 {
    // Device encodes gain as integer tenths of a dB, stored as i8.
    // Range: ±12 dB → ±120, which fits in i8 (−128..127).
    (gain * 10.0).clamp(-127.0, 127.0).round() as i8
}

/// Encode a single EQ band as the 4-byte on-wire format:
/// `[freq_lo, freq_hi, filter_type, gain_i8]`.
fn encode_band_bytes(band: &EqBand) -> [u8; 4] {
    let freq = band.frequency.unwrap_or(1000);
    let ft = band.filter_type.map_or(2, filter_type_byte); // default: Peaking
    let g = gain_i8_from_f32(band.gain) as u8;
    let [fl, fh] = freq.to_le_bytes();
    [fl, fh, ft, g]
}

/// Encode a 10-band parametric EQ payload for Nova 3/5/7 Gen2 family.
///
/// Layout: `[report_id, cmd, band1..band10 (4 bytes each), 0x00 × padding]`
/// padded to 65 bytes.  Pass `rf_cmd` for 2.4 GHz, `bt_cmd` for Bluetooth.
pub fn encode_parametric_nova_eq(report_id: u8, cmd: u8, bands: &[EqBand]) -> Vec<u8> {
    let mut buf = vec![0u8; 65];
    buf[0] = report_id;
    buf[1] = cmd;
    for (i, band) in bands.iter().take(10).enumerate() {
        let bytes = encode_band_bytes(band);
        buf[2 + i * 4..2 + i * 4 + 4].copy_from_slice(&bytes);
    }
    buf
}

// ── Public encode dispatcher ──────────────────────────────────────────────────

/// Encode EQ payloads ready for HID transmission.
///
/// Returns one payload for NovaPro (EQ data), or one payload for ParametricNova.
/// NovaPro also requires a preset-select follow-up — call `encode_preset_select`
/// after sending the first payload.
pub fn encode_eq_payloads(cap: &HwEqCapability, bands: &[EqBand]) -> Vec<Vec<u8>> {
    match cap.format {
        HwEqFormat::NovaPro { report_id, eq_cmd, .. } => {
            vec![encode_nova_pro_eq(report_id, eq_cmd, bands)]
        }
        HwEqFormat::ParametricNova { report_id, rf_cmd, .. } => {
            vec![encode_parametric_nova_eq(report_id, rf_cmd, bands)]
        }
    }
}

/// Encode the follow-up preset-select payload for NovaPro devices.
/// Returns `None` for devices that don't need it.
pub fn encode_preset_select(cap: &HwEqCapability) -> Option<Vec<u8>> {
    if let HwEqFormat::NovaPro { report_id, preset_cmd, .. } = cap.format {
        Some(encode_nova_pro_preset_select(report_id, preset_cmd))
    } else {
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::preset::EqBand;

    fn flat_10() -> Vec<EqBand> {
        (0..10).map(|_| EqBand::gain_only(0.0)).collect()
    }

    fn parametric_10() -> Vec<EqBand> {
        let freqs: [u16; 10] = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
        freqs
            .iter()
            .map(|&f| EqBand::parametric(f, FilterType::Peaking, 0.0))
            .collect()
    }

    // ── NovaPro gain encoding ─────────────────────────────────────────────────

    #[test]
    fn nova_pro_gain_zero_db_is_20() {
        assert_eq!(nova_pro_gain_byte(0.0), 20);
    }

    #[test]
    fn nova_pro_gain_plus10_is_40() {
        assert_eq!(nova_pro_gain_byte(10.0), 40);
    }

    #[test]
    fn nova_pro_gain_minus10_is_0() {
        assert_eq!(nova_pro_gain_byte(-10.0), 0);
    }

    #[test]
    fn nova_pro_gain_clamps_above_10() {
        assert_eq!(nova_pro_gain_byte(15.0), 40);
    }

    #[test]
    fn nova_pro_gain_clamps_below_minus10() {
        assert_eq!(nova_pro_gain_byte(-15.0), 0);
    }

    #[test]
    fn nova_pro_gain_plus5_is_30() {
        assert_eq!(nova_pro_gain_byte(5.0), 30);
    }

    // ── NovaPro payload structure ─────────────────────────────────────────────

    #[test]
    fn encode_nova_pro_eq_header_and_padding() {
        let buf = encode_nova_pro_eq(0x06, 0x33, &flat_10());
        assert_eq!(buf.len(), 64);
        assert_eq!(buf[0], 0x06);
        assert_eq!(buf[1], 0x33);
        // All 10 gains = 0 dB → byte 20
        for i in 2..12 {
            assert_eq!(buf[i], 20, "gain byte at {i} should be 20");
        }
        // Remainder padded with 0x00
        for i in 12..64 {
            assert_eq!(buf[i], 0, "padding at {i} should be 0");
        }
    }

    #[test]
    fn encode_nova_pro_eq_gain_values_correct() {
        let bands = vec![
            EqBand::gain_only(10.0),  // 40
            EqBand::gain_only(-10.0), // 0
            EqBand::gain_only(0.0),   // 20
            EqBand::gain_only(5.0),   // 30
            EqBand::gain_only(-5.0),  // 10
            EqBand::gain_only(2.0),   // 24
            EqBand::gain_only(-2.0),  // 16
            EqBand::gain_only(1.0),   // 22
            EqBand::gain_only(-1.0),  // 18
            EqBand::gain_only(0.5),   // 21
        ];
        let buf = encode_nova_pro_eq(0x06, 0x33, &bands);
        assert_eq!(buf[2], 40);
        assert_eq!(buf[3], 0);
        assert_eq!(buf[4], 20);
        assert_eq!(buf[5], 30);
        assert_eq!(buf[6], 10);
        assert_eq!(buf[7], 24);
        assert_eq!(buf[8], 16);
        assert_eq!(buf[9], 22);
        assert_eq!(buf[10], 18);
        assert_eq!(buf[11], 21);
    }

    #[test]
    fn encode_nova_pro_preset_select_is_custom_slot() {
        let buf = encode_nova_pro_preset_select(0x06, 0x2E);
        assert_eq!(buf.len(), 64);
        assert_eq!(buf[0], 0x06);
        assert_eq!(buf[1], 0x2E);
        assert_eq!(buf[2], 0x04); // Custom EQ slot
    }

    // ── ParametricNova band encoding ──────────────────────────────────────────

    #[test]
    fn gain_i8_zero_is_zero() {
        assert_eq!(gain_i8_from_f32(0.0), 0);
    }

    #[test]
    fn gain_i8_plus12_is_120() {
        assert_eq!(gain_i8_from_f32(12.0), 120);
    }

    #[test]
    fn gain_i8_minus12_is_minus120() {
        assert_eq!(gain_i8_from_f32(-12.0), -120);
    }

    #[test]
    fn gain_i8_plus3_5_is_35() {
        assert_eq!(gain_i8_from_f32(3.5), 35);
    }

    #[test]
    fn encode_band_bytes_frequency_little_endian() {
        let band = EqBand::parametric(1000, FilterType::Peaking, 0.0);
        let bytes = encode_band_bytes(&band);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 1000);
    }

    #[test]
    fn encode_band_bytes_filter_type_low_shelf() {
        let band = EqBand::parametric(80, FilterType::LowShelf, 3.5);
        let bytes = encode_band_bytes(&band);
        assert_eq!(bytes[2], 1); // LowShelf = 1
        assert_eq!(bytes[3] as i8, 35); // 3.5 * 10
    }

    #[test]
    fn encode_band_bytes_filter_type_high_shelf() {
        let band = EqBand::parametric(16000, FilterType::HighShelf, -2.0);
        let bytes = encode_band_bytes(&band);
        assert_eq!(bytes[2], 3); // HighShelf = 3
        assert_eq!(bytes[3] as i8, -20);
    }

    // ── ParametricNova payload structure ──────────────────────────────────────

    #[test]
    fn encode_parametric_nova_eq_length_and_header() {
        let buf = encode_parametric_nova_eq(0x00, 0x33, &parametric_10());
        assert_eq!(buf.len(), 65);
        assert_eq!(buf[0], 0x00);
        assert_eq!(buf[1], 0x33);
    }

    #[test]
    fn encode_parametric_nova_eq_bt_uses_bt_cmd() {
        let bands = parametric_10();
        let rf = encode_parametric_nova_eq(0x00, 0x33, &bands);
        let bt = encode_parametric_nova_eq(0x00, 0x34, &bands);
        assert_eq!(rf[1], 0x33);
        assert_eq!(bt[1], 0x34);
        // Band data identical
        assert_eq!(rf[2..42], bt[2..42]);
    }

    #[test]
    fn encode_parametric_nova_eq_band_layout() {
        let band = EqBand::parametric(500, FilterType::Peaking, 6.0);
        let bands: Vec<EqBand> = std::iter::repeat(band).take(10).collect();
        let buf = encode_parametric_nova_eq(0x00, 0x33, &bands);
        // First band at offset 2
        let [fl, fh, ft, g] = [buf[2], buf[3], buf[4], buf[5]];
        assert_eq!(u16::from_le_bytes([fl, fh]), 500);
        assert_eq!(ft, 2); // Peaking
        assert_eq!(g as i8, 60); // 6.0 * 10
    }

    #[test]
    fn encode_parametric_nova_eq_padding_zeroes() {
        let buf = encode_parametric_nova_eq(0x00, 0x33, &parametric_10());
        // 10 bands × 4 bytes = 40 bytes at offsets 2..42; rest must be 0
        for i in 42..65 {
            assert_eq!(buf[i], 0, "padding at {i} should be 0");
        }
    }

    // ── Dispatcher ────────────────────────────────────────────────────────────

    #[test]
    fn encode_eq_payloads_nova_pro_returns_one_payload() {
        let cap = HwEqCapability::nova_pro_wireless();
        let payloads = encode_eq_payloads(&cap, &flat_10());
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].len(), 64);
        assert_eq!(payloads[0][0], 0x06);
        assert_eq!(payloads[0][1], 0x33);
    }

    #[test]
    fn encode_preset_select_nova_pro_returns_some() {
        let cap = HwEqCapability::nova_pro_wireless();
        let sel = encode_preset_select(&cap);
        assert!(sel.is_some());
        let sel = sel.unwrap();
        assert_eq!(sel[0], 0x06);
        assert_eq!(sel[1], 0x2E);
        assert_eq!(sel[2], 0x04);
    }

    #[test]
    fn encode_eq_payloads_parametric_returns_one_payload() {
        let cap = HwEqCapability::nova_parametric();
        let payloads = encode_eq_payloads(&cap, &parametric_10());
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].len(), 65);
    }

    #[test]
    fn encode_preset_select_parametric_returns_none() {
        let cap = HwEqCapability::nova_parametric();
        assert!(encode_preset_select(&cap).is_none());
    }
}
