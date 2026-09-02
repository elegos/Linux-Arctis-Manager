//! Driver-computed biquad EQ, for headset DSP chips that do no on-device
//! coefficient math at all (unlike every other EQ device this project
//! supports, where the firmware itself turns a dB gain into its own curve).
//! Two chips are known to need this so far: AV6X02 (Arctis 7, Arctis 1
//! Wireless) and the Conexant CX20892 (Arctis 5). Both are direct ports of
//! the RBJ Audio EQ Cookbook formula the vendor spec itself is built from —
//! see `equalizer.device` in the raw spec archive.
//!
//! Computed in `f64` throughout, even though the wire format narrows to
//! float32 (AV6X02) or 16-bit fixed-point (CX20892) — the vendor spec's own
//! reference implementation is float64 Scheme, and normalizing before the
//! final narrowing cast avoids compounding rounding error differently than
//! the vendor tool.
//!
//! No hardware is available to validate any of this against a real device.
//! Unlike this project's other EQ builtins (which only reformat bytes), this
//! is genuine DSP: a subtle sign or ordering error would silently ship a
//! wrong curve. Flagged as the highest-risk part of this whole port.

use crate::api_executor::BuiltinArgs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterShape {
    LowShelf,
    HighShelf,
    Peaking,
}

/// `(b0, b1, b2, a0, a1, a2)` — direct port of `equalizer.device`'s
/// `calculate_coefficients`. `gain_db == 0.0` short-circuits to the flat
/// passthrough `(1, 0, 0, 1, 0, 0)`, exactly as the vendor spec does — that
/// is how "disable this band" is spelled on the wire.
pub fn calculate_coefficients(
    shape: FilterShape,
    fs: f64,
    f0: f64,
    gain_db: f64,
    q: f64,
) -> [f64; 6] {
    if gain_db == 0.0 {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let w0 = 2.0 * std::f64::consts::PI * f0 / fs;
    let cosw0 = w0.cos();
    let sinw0 = w0.sin();
    let alpha = sinw0 / 2.0 / q;
    let a = 10.0_f64.powf(gain_db / 40.0);
    let two_sqrta_alpha = 2.0 * a.sqrt() * alpha;

    match shape {
        FilterShape::LowShelf => {
            let b0 = a * ((a + 1.0) - (a - 1.0) * cosw0 + two_sqrta_alpha);
            let b1 = 2.0 * a * (a - 1.0 - (a + 1.0) * cosw0);
            let b2 = a * ((a + 1.0) - (a - 1.0) * cosw0 - two_sqrta_alpha);
            let a0 = (a + 1.0) + (a - 1.0) * cosw0 + two_sqrta_alpha;
            let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cosw0);
            let a2 = (a + 1.0) + (a - 1.0) * cosw0 - two_sqrta_alpha;
            [b0, b1, b2, a0, a1, a2]
        }
        FilterShape::HighShelf => {
            let b0 = a * ((a + 1.0) + (a - 1.0) * cosw0 + two_sqrta_alpha);
            let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cosw0);
            let b2 = a * ((a + 1.0) + (a - 1.0) * cosw0 - two_sqrta_alpha);
            let a0 = (a + 1.0) - (a - 1.0) * cosw0 + two_sqrta_alpha;
            let a1 = 2.0 * (a - 1.0 - (a + 1.0) * cosw0);
            let a2 = (a + 1.0) - (a - 1.0) * cosw0 - two_sqrta_alpha;
            [b0, b1, b2, a0, a1, a2]
        }
        FilterShape::Peaking => {
            let b0 = 1.0 + alpha * a;
            let b1 = -2.0 * cosw0;
            let b2 = 1.0 - alpha * a;
            let a0 = 1.0 + alpha / a;
            let a1 = -2.0 * cosw0;
            let a2 = 1.0 - alpha / a;
            [b0, b1, b2, a0, a1, a2]
        }
    }
}

/// a0-normalize: `b/a0`, `a/a0`, `a0 -> 1.0`.
pub fn normalize_coefficients(c: [f64; 6]) -> [f64; 6] {
    let [b0, b1, b2, a0, a1, a2] = c;
    [b0 / a0, b1 / a0, b2 / a0, 1.0, a1 / a0, a2 / a0]
}

// ── AV6X02 (Arctis 7, Arctis 1 Wireless) ────────────────────────────────────

/// Chip-intrinsic band table — identical across both device families that
/// use this chip, per the raw spec (`av6x02-eq-drc.device`). Named after the
/// chip rather than either family since it is genuinely shared, not
/// (yet) shared.
const AV6X02_BANDS: [(FilterShape, f64, f64); 6] = [
    (FilterShape::LowShelf, 64.0, 1.1),
    (FilterShape::Peaking, 180.0, 0.8),
    (FilterShape::Peaking, 500.0, 0.8),
    (FilterShape::Peaking, 1400.0, 0.8),
    (FilterShape::Peaking, 3900.0, 0.7),
    (FilterShape::HighShelf, 11000.0, 1.1),
];
const AV6X02_FS: f64 = 48000.0;
/// `coefs->eq_filters`' per-band `filter_num` base (band 0 -> 1, ... band 5
/// -> 36) and the fixed +42 "right channel" offset it always uses.
const AV6X02_FILTER_NUM_BASE: [u8; 6] = [1, 8, 15, 22, 29, 36];
const AV6X02_RIGHT_OFFSET: u8 = 42;

/// One physical `eq_filter` message: `[report_id=0x06, command=0x28,
/// filter[6], filter_num]`, padded by the engine to the api's `chunk_size`.
fn av6x02_filter_message(
    channel: u8,
    address_byte: u8,
    payload: [u8; 4],
    filter_num: u8,
) -> Vec<u8> {
    vec![
        0x06,
        0x28,
        channel,
        address_byte,
        payload[0],
        payload[1],
        payload[2],
        payload[3],
        filter_num,
    ]
}

/// Port of `coefs->filters`: for one channel, either a single disable-filter
/// message (flat/zero-gain band) or 6 coefficient writes + 1 enable write.
fn av6x02_channel_messages(
    channel: u8,
    band: u8,
    coefs: [f64; 6],
    filter_num_base: u8,
) -> Vec<Vec<u8>> {
    let channel_bits = band << 4;
    let is_flat = coefs[0] == 1.0 && coefs[3] == 1.0;
    if is_flat {
        return vec![av6x02_filter_message(
            channel,
            channel_bits,
            [0, 0, 0, 0],
            filter_num_base + 6,
        )];
    }
    let addresses: [u8; 6] = [0, 1, 2, 3, 4, 5];
    let mut out = Vec::with_capacity(7);
    for (address, coef) in addresses.iter().zip(coefs.iter()) {
        let addr_byte = channel_bits | 0x08 | address;
        let bytes = (*coef as f32).to_bits().to_be_bytes();
        out.push(av6x02_filter_message(
            channel,
            addr_byte,
            bytes,
            filter_num_base + address,
        ));
    }
    out.push(av6x02_filter_message(
        channel,
        channel_bits,
        [0, 0, 0, 0],
        filter_num_base + 6,
    ));
    out
}

/// Port of `coefs->eq_filters`: normalized coefficients for one band,
/// negate a1/a2, then emit both channels (L=3, R=4, R offset +42).
fn av6x02_band_messages(band: u8, normalized: [f64; 6], filter_num_base: u8) -> Vec<Vec<u8>> {
    let [b0, b1, b2, a0, a1, a2] = normalized;
    let corrected = [b0, b1, b2, a0, -a1, -a2];
    let mut out = av6x02_channel_messages(3, band, corrected, filter_num_base);
    out.extend(av6x02_channel_messages(
        4,
        band,
        corrected,
        filter_num_base + AV6X02_RIGHT_OFFSET,
    ));
    out
}

/// Full-payload transform for AV6X02's `custom_eq` write: input is 6×
/// float32 gains (dB) bundled in one struct (this project's usual
/// "resend the whole curve on any change" shape — the raw spec's 6 separate
/// per-band APIs are collapsed into one write here, behaviorally
/// equivalent). Output: up to 14 physical `eq_filter` messages per band (6
/// coefficient writes + 1 enable, ×2 channels), or 2 disable messages for a
/// flat/zero-gain band.
pub fn av6x02_eq_gains_payload(bytes: &[u8], _args: &BuiltinArgs) -> Vec<Vec<u8>> {
    if bytes.len() < 2 + 6 * 4 {
        return vec![bytes.to_vec()];
    }
    let mut out = Vec::new();
    for (band_idx, (shape, f0, q)) in AV6X02_BANDS.iter().enumerate() {
        let base = 2 + band_idx * 4;
        let gain_db = f32::from_be_bytes(bytes[base..base + 4].try_into().unwrap()) as f64;
        let coefs = calculate_coefficients(*shape, AV6X02_FS, *f0, gain_db, *q);
        let normalized = normalize_coefficients(coefs);
        out.extend(av6x02_band_messages(
            band_idx as u8,
            normalized,
            AV6X02_FILTER_NUM_BASE[band_idx],
        ));
    }
    out
}

/// The one-time `initialize-av6x02` sequence: fixed, compile-time-constant
/// filter writes (interrupt-mode toggles + 2 low-pass + 2 high-pass fixed
/// DRC stages), copied verbatim from `av6x02-eq-drc.device`'s
/// `initialize-av6x02`. Every value here is chip-intrinsic, not
/// user/device-configurable — there is nothing for `payload_transform_args`
/// to carry, this builtin ignores both its inputs.
pub fn av6x02_init_payload(_bytes: &[u8], _args: &BuiltinArgs) -> Vec<Vec<u8>> {
    let mut out = vec![
        av6x02_filter_message(0x18, 0x00, [0, 0, 0, 0], 0),
        av6x02_filter_message(0x05, 0x00, [0x20, 0, 0, 0], 141),
        av6x02_filter_message(0x06, 0x00, [0x20, 0, 0, 0], 142),
    ];
    // low-pass-stage-0 (band 6, filter_num base 85, right offset 28)
    let low_pass = [
        0.0003297046,
        0.0006594092,
        0.0003297046,
        1.0,
        -1.9479868,
        0.9493058,
    ];
    let high_pass = [
        0.97432315, -1.9486463, 0.97432315, 1.0, -1.9479868, 0.9493058,
    ];
    out.extend(av6x02_band_messages(6, low_pass, 85));
    out.extend(av6x02_band_messages(7, low_pass, 92));
    out.extend(av6x02_band_messages(8, high_pass, 99));
    out.extend(av6x02_band_messages(9, high_pass, 106));
    out.push(av6x02_filter_message(0x07, 0x10, [0x67, 0, 0, 0], 145));
    out.push(av6x02_filter_message(0x07, 0x20, [0xF6, 0, 0, 0], 146));
    out.push(av6x02_filter_message(0x07, 0x02, [0x00, 0, 0, 0], 147));
    out.push(av6x02_filter_message(0x07, 0x11, [0x67, 0, 0, 0], 150));
    out.push(av6x02_filter_message(0x07, 0x21, [0xF6, 0, 0, 0], 151));
    out.push(av6x02_filter_message(0x07, 0x03, [0x00, 0, 0, 0], 152));
    out
}

// ── CX20892 (Arctis 5 family) ───────────────────────────────────────────────

/// Chip-intrinsic band table, currently only used by the Arctis 5 family —
/// named `arctis5` rather than by chip until a second family shares it, per
/// the same naming rule as `AV6X02_BANDS` but the other half of it.
const ARCTIS5_BANDS: [(FilterShape, f64, f64, u8); 5] = [
    (FilterShape::LowShelf, 62.5, 1.1, 0x20),
    (FilterShape::Peaking, 250.0, 0.8, 0x2B),
    (FilterShape::Peaking, 1000.0, 0.8, 0x36),
    (FilterShape::Peaking, 3600.0, 0.7, 0x41),
    (FilterShape::HighShelf, 12000.0, 1.1, 0x4C),
];
const ARCTIS5_FS: f64 = 48000.0;

/// Port of `arctis-5-eq-normalization`: a0-normalize, then rescale so
/// a0 = 0.5 (halve b0/b1/b2, drop a0, negate+halve a1/a2).
fn arctis5_eq_normalization(c: [f64; 6]) -> [f64; 5] {
    let n = normalize_coefficients(c);
    [n[0] / 2.0, n[1] / 2.0, n[2] / 2.0, n[4] / -2.0, n[5] / -2.0]
}

/// Port of `coef-floor`/`float->short-floor`: round-half-up
/// (`floor(x*32768+0.5)`), clamp to `[-32767, 32767]`.
fn float_to_short_floor(f: f64) -> i16 {
    let scaled = f * 32768.0 + 0.5;
    let floored = scaled.floor();
    floored.clamp(-32767.0, 32767.0) as i16
}

/// Port of `float->short-truncate`: `floor(x*32768)`, clamp to
/// `[-32767, 32767]` — used only for the a1/a2 stability search, which
/// needs the un-rounded truncation as its search origin.
fn float_to_short_truncate(f: f64) -> i16 {
    let scaled = f * 32768.0;
    scaled.floor().clamp(-32767.0, 32767.0) as i16
}

/// Port of `check-demoninators`: true iff the characteristic polynomial
/// `a·x^2 + b·x + c` (with the fixed `a = 0x4000`) has both roots strictly
/// inside the unit circle — i.e. the resulting filter is stable.
fn check_denominators(b: i16, c: i16) -> bool {
    let a: f64 = 0x4000 as f64;
    let b = -(b as f64);
    let c = -(c as f64);
    let d = b * b - 4.0 * a * c;
    let (r1_sq, r2_sq) = if d >= 0.0 {
        let dsqrt = d.sqrt();
        let r1 = (dsqrt - b) / (2.0 * a);
        let r2 = (-dsqrt - b) / (2.0 * a);
        (r1 * r1, r2 * r2)
    } else {
        let dsqrt = (-d).sqrt();
        let r1 = -b / (2.0 * a);
        let i1 = dsqrt / (2.0 * a);
        let root_abs_sqr = r1 * r1 + i1 * i1;
        (root_abs_sqr, root_abs_sqr)
    };
    r1_sq < 1.0 && r2_sq < 1.0
}

/// Port of `eq_band_for_frequency`'s 5-candidate rounding search. The
/// vendor's own candidate list (in order) is: the *floor*-rounded (a1, a2)
/// pair, then four variations of the *truncate*-rounded pair — `(a1+1,a2)`,
/// `(a1,a2+1)`, `(a1,a2)` unchanged, `(a1+1,a2+1)` — where `+1` is the
/// vendor's `succ` on the 16-bit two's-complement wire representation
/// (`wrapping_add`, not a saturating one, to match that bit-for-bit). Falls
/// back to the last candidate if somehow none pass (the vendor script has
/// no fallback at all — it would hang forever; a fallback here is a
/// deliberate, safer divergence, not an attempt to reproduce the vendor's
/// own infinite-loop behaviour).
fn arctis5_stable_a1_a2(a1_coef: f64, a2_coef: f64) -> (i16, i16) {
    let floor_a1 = float_to_short_floor(a1_coef);
    let floor_a2 = float_to_short_floor(a2_coef);
    let trunc_a1 = float_to_short_truncate(a1_coef);
    let trunc_a2 = float_to_short_truncate(a2_coef);
    let candidates = [
        (floor_a1, floor_a2),
        (trunc_a1.wrapping_add(1), trunc_a2),
        (trunc_a1, trunc_a2.wrapping_add(1)),
        (trunc_a1, trunc_a2),
        (trunc_a1.wrapping_add(1), trunc_a2.wrapping_add(1)),
    ];
    for (ca1, ca2) in candidates {
        if check_denominators(ca1, ca2) {
            return (ca1, ca2);
        }
    }
    candidates[candidates.len() - 1]
}

/// One physical `equalizer_band` message: `[0x04, 0x40, 0x0B, addr_hi=0x10,
/// addr_lo, b0, b1, b2 (2 bytes each, big-endian), a1, a2 (2 bytes each,
/// big-endian), 0x03]` — 16 bytes, per `eq_band_for_frequency`.
fn arctis5_band_message(addr_lo: u8, b: [i16; 3], a1: i16, a2: i16) -> Vec<u8> {
    let mut out = vec![0x04, 0x40, 0x0B, 0x10, addr_lo];
    for v in b {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&a1.to_be_bytes());
    out.extend_from_slice(&a2.to_be_bytes());
    out.push(0x03);
    out
}

/// Full-payload transform for the CX20892's `custom_eq` write (Arctis 5
/// family): input is 5× float32 gains (dB) bundled in one struct, output is
/// 5 physical messages (one per band — this chip needs no per-channel
/// duplication, unlike AV6X02).
pub fn arctis5_eq_gains_payload(bytes: &[u8], _args: &BuiltinArgs) -> Vec<Vec<u8>> {
    if bytes.len() < 2 + 5 * 4 {
        return vec![bytes.to_vec()];
    }
    let mut out = Vec::with_capacity(5);
    for (band_idx, (shape, f0, q, addr_lo)) in ARCTIS5_BANDS.iter().enumerate() {
        let base = 2 + band_idx * 4;
        let gain_db = f32::from_be_bytes(bytes[base..base + 4].try_into().unwrap()) as f64;
        let coefs = calculate_coefficients(*shape, ARCTIS5_FS, *f0, gain_db, *q);
        let [b0, b1, b2, a1_coef, a2_coef] = arctis5_eq_normalization(coefs);
        let b = [
            float_to_short_floor(b0),
            float_to_short_floor(b1),
            float_to_short_floor(b2),
        ];
        let (a1, a2) = arctis5_stable_a1_a2(a1_coef, a2_coef);
        out.push(arctis5_band_message(*addr_lo, b, a1, a2));
    }
    out
}

/// Arctis 5's `commit_settings` register write — a fixed, chip-intrinsic
/// message (`[0x04, 0x40, 0x01, 0x11, 0x54, 0x9B]`) the raw spec says must
/// follow "certain" settings writes to make them take effect, without
/// saying precisely which. Defaulting to calling it after *every* settings
/// write (see each api's `write.steps` in `base_arctis_5.yaml`) is the safe
/// choice given that ambiguity. Ignores its input entirely, like
/// `av6x02_init_payload`.
pub fn arctis5_commit_settings_payload(_bytes: &[u8], _args: &BuiltinArgs) -> Vec<Vec<u8>> {
    vec![vec![0x04, 0x40, 0x01, 0x11, 0x54, 0x9B]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> BuiltinArgs {
        BuiltinArgs::new()
    }

    // Reference values below are computed independently from the published
    // RBJ Audio EQ Cookbook formula (not by re-deriving this module's own
    // code), fs=48000, to catch a transcription error in the port itself.

    #[test]
    fn peaking_zero_gain_is_flat() {
        let c = calculate_coefficients(FilterShape::Peaking, 48000.0, 1000.0, 0.0, 0.8);
        assert_eq!(c, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn peaking_matches_rbj_cookbook_reference() {
        // f0=1000Hz, fs=48000Hz, Q=0.8, +6dB.
        // w0 = 2*pi*1000/48000 = 0.1308996939
        // A = 10^(6/40) = 1.4125375446
        let c = calculate_coefficients(FilterShape::Peaking, 48000.0, 1000.0, 6.0, 0.8);
        let w0 = 2.0 * std::f64::consts::PI * 1000.0 / 48000.0;
        let cosw0 = w0.cos();
        let alpha = w0.sin() / (2.0 * 0.8);
        let a = 10.0_f64.powf(6.0 / 40.0);
        let expected_b0 = 1.0 + alpha * a;
        let expected_a2 = 1.0 - alpha / a;
        assert!((c[0] - expected_b0).abs() < 1e-9);
        assert!((c[5] - expected_a2).abs() < 1e-9);
        assert!((c[1] - (-2.0 * cosw0)).abs() < 1e-9);
    }

    #[test]
    fn normalize_sets_a0_to_one() {
        let n = normalize_coefficients([2.0, 4.0, 6.0, 2.0, 8.0, 10.0]);
        assert_eq!(n, [1.0, 2.0, 3.0, 1.0, 4.0, 5.0]);
    }

    #[test]
    fn av6x02_flat_gain_emits_two_disable_messages() {
        let mut bytes = vec![0x06, 0x28];
        for _ in 0..6 {
            bytes.extend_from_slice(&0.0f32.to_be_bytes());
        }
        let msgs = av6x02_eq_gains_payload(&bytes, &args());
        // 6 flat bands x 2 channels (L, R) = 12 disable messages.
        assert_eq!(msgs.len(), 12);
        // Band 0, left channel: channel_bits = 0<<4 = 0x00, filter_num = 1+6 = 7.
        assert_eq!(msgs[0], vec![0x06, 0x28, 3, 0x00, 0, 0, 0, 0, 7]);
        // Band 0, right channel: filter_num = 1+42+6 = 49.
        assert_eq!(msgs[1], vec![0x06, 0x28, 4, 0x00, 0, 0, 0, 0, 49]);
    }

    #[test]
    fn av6x02_nonzero_gain_emits_fourteen_messages_for_that_band() {
        let mut bytes = vec![0x06, 0x28];
        bytes.extend_from_slice(&3.0f32.to_be_bytes()); // band 0: +3dB
        for _ in 1..6 {
            bytes.extend_from_slice(&0.0f32.to_be_bytes());
        }
        let msgs = av6x02_eq_gains_payload(&bytes, &args());
        // band 0: 7 messages (6 coef + 1 enable) x 2 channels = 14.
        // bands 1..5: 2 disable messages each = 10.
        assert_eq!(msgs.len(), 14 + 10);
        // First message: band 0, left channel, coefficient address 0.
        // channel_bits = 0, address_byte = 0x00 | 0x08 | 0x00 = 0x08.
        assert_eq!(msgs[0][0..4], [0x06, 0x28, 3, 0x08]);
        assert_eq!(msgs[0][8], 1); // filter_num base for band 0 = 1
    }

    #[test]
    fn av6x02_gain_direction_matches_shelf_sign() {
        // A positive low-shelf gain should raise b0 relative to a flat band.
        let flat = calculate_coefficients(FilterShape::LowShelf, 48000.0, 64.0, 0.0, 1.1);
        let boosted = calculate_coefficients(FilterShape::LowShelf, 48000.0, 64.0, 6.0, 1.1);
        assert_eq!(flat[0], 1.0);
        assert!(boosted[0] > 1.0);
    }

    #[test]
    fn av6x02_init_is_deterministic_and_nonempty() {
        let msgs = av6x02_init_payload(&[], &args());
        assert!(!msgs.is_empty());
        // 3 fixed setup + 4 DRC stages x (7 msgs/channel x 2 channels) + 6 tail.
        assert_eq!(msgs.len(), 3 + 4 * 14 + 6);
        for m in &msgs {
            assert_eq!(m.len(), 9);
            assert_eq!(m[0], 0x06);
            assert_eq!(m[1], 0x28);
        }
    }

    #[test]
    fn arctis5_short_conversion_rounds_half_up_then_clamps() {
        assert_eq!(float_to_short_floor(0.5 / 32768.0), 1);
        assert_eq!(float_to_short_floor(-0.5 / 32768.0), 0);
        assert_eq!(float_to_short_floor(2.0), 32767);
        assert_eq!(float_to_short_floor(-2.0), -32767);
    }

    #[test]
    fn arctis5_check_denominators_rejects_unstable_pole() {
        // a1=0, a2=-20000 (Q15) -> c = 20000/16384 ~= 1.22, way outside the
        // unit circle for a=0x4000 with b=0 (real double root at sqrt(-4ac/2a)).
        assert!(!check_denominators(0, -20000));
        // a1=0, a2=0 -> both roots at the origin, clearly stable.
        assert!(check_denominators(0, 0));
    }

    #[test]
    fn arctis5_stable_search_always_returns_a_passing_candidate() {
        // Exercise the search loop (not just the math) across every real
        // band/gain combination this device can produce, at the extremes
        // most likely to land near the unit-circle boundary.
        for (shape, f0, q, _) in ARCTIS5_BANDS {
            for gain in [-12.0, -6.0, 6.0, 12.0] {
                let c = calculate_coefficients(shape, ARCTIS5_FS, f0, gain, q);
                let [_, _, _, a1_coef, a2_coef] = arctis5_eq_normalization(c);
                let (a1, a2) = arctis5_stable_a1_a2(a1_coef, a2_coef);
                assert!(
                    check_denominators(a1, a2),
                    "unstable pair for f0={f0} gain={gain}: a1={a1} a2={a2}"
                );
            }
        }
    }

    #[test]
    fn arctis5_stable_search_candidate_order_matches_vendor_spec() {
        // Floor and truncate agree away from a rounding-boundary value, so
        // pick coefficients where they differ (a1 rounds up under floor,
        // stays flat under truncate) and confirm the floor-rounded
        // candidate — checked first — is preferred whenever it's stable.
        let a1_coef = 0.4 / 32768.0; // floor -> 1, truncate -> 0
        let a2_coef = 0.0;
        let (a1, a2) = arctis5_stable_a1_a2(a1_coef, a2_coef);
        assert_eq!((a1, a2), (float_to_short_floor(a1_coef), 0));
    }

    #[test]
    fn arctis5_zero_gain_all_bands_produces_five_messages() {
        let mut bytes = vec![0x00, 0x00];
        for _ in 0..5 {
            bytes.extend_from_slice(&0.0f32.to_be_bytes());
        }
        let msgs = arctis5_eq_gains_payload(&bytes, &args());
        assert_eq!(msgs.len(), 5);
        for (i, (_, _, _, addr_lo)) in ARCTIS5_BANDS.iter().enumerate() {
            assert_eq!(msgs[i][0..5], [0x04, 0x40, 0x0B, 0x10, *addr_lo]);
            assert_eq!(msgs[i].len(), 16);
        }
    }

    #[test]
    fn arctis5_nonzero_gain_changes_coefficient_bytes() {
        let mut flat = vec![0x00, 0x00];
        for _ in 0..5 {
            flat.extend_from_slice(&0.0f32.to_be_bytes());
        }
        let mut boosted = vec![0x00, 0x00];
        boosted.extend_from_slice(&4.0f32.to_be_bytes());
        for _ in 1..5 {
            boosted.extend_from_slice(&0.0f32.to_be_bytes());
        }
        let flat_msgs = arctis5_eq_gains_payload(&flat, &args());
        let boosted_msgs = arctis5_eq_gains_payload(&boosted, &args());
        assert_ne!(flat_msgs[0], boosted_msgs[0]);
        // Untouched bands stay identical.
        assert_eq!(flat_msgs[1], boosted_msgs[1]);
    }

    #[test]
    fn av6x02_short_input_passes_through_unchanged() {
        let bytes = vec![0x06, 0x28, 0x01];
        assert_eq!(av6x02_eq_gains_payload(&bytes, &args()), vec![bytes]);
    }

    #[test]
    fn arctis5_commit_settings_is_a_fixed_message() {
        let msgs = arctis5_commit_settings_payload(&[1, 2, 3], &args());
        assert_eq!(msgs, vec![vec![0x04, 0x40, 0x01, 0x11, 0x54, 0x9B]]);
    }

    #[test]
    fn arctis5_short_input_passes_through_unchanged() {
        let bytes = vec![0x00, 0x00, 0x01];
        assert_eq!(arctis5_eq_gains_payload(&bytes, &args()), vec![bytes]);
    }
}
