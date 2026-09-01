/// Converts 10 IEEE 754 big-endian float32 gain values (in dB) into 10 uint8
/// firmware values: `firmware_val = clamp(round(2 × (10 + gain_dB)), 0, 255) as u8`.
///
/// Input: 40 bytes (10 × f32 BE — matching `codec::write_fv`'s big-endian
/// encoding of every multi-byte numeric field).  Output: 10 bytes, one per
/// EQ band.
pub fn gains_to_firmware_values(bytes: &[u8]) -> Vec<u8> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| {
            let db = f32::from_be_bytes(*c);
            ((2.0_f32 * (10.0_f32 + db)).round() as i32).clamp(0, 255) as u8
        })
        .collect()
}

/// Full-payload transform for `custom_eq` write: passes `report_id` and `command`
/// through unchanged, then converts 10 × float32 gains (bytes 2–41) to 10 × uint8
/// firmware values using [`gains_to_firmware_values`].
///
/// Input:  `[report_id, command, gain1_bytes[4], …, gain10_bytes[4], …]` (42+ bytes).
/// Output: one packet `[report_id, command, fw_val1, …, fw_val10]`.
pub fn custom_eq_gains_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() < 2 {
        return vec![bytes.to_vec()];
    }
    let mut out = Vec::with_capacity(12);
    out.push(bytes[0]);
    out.push(bytes[1]);
    out.extend_from_slice(&gains_to_firmware_values(&bytes[2..]));
    vec![out]
}

/// Full-payload transform for `high_gain` write.
/// Maps byte 2: 0 (disabled / low gain) → 1, 1 (enabled / high gain) → 2.
pub fn high_gain_write_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = bytes.to_vec();
    if out.len() >= 3 {
        out[2] = match out[2] {
            0 => 1,
            1 => 2,
            v => v,
        };
    }
    vec![out]
}

/// Full-payload transform for `dim_timer` write.
/// Converts byte 2 from user-facing minutes (0, 1, 5, 10, 15, 30, 60) to the
/// firmware enum value (0–6).  Unknown minute values map to 0 (never).
pub fn dim_timer_write_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = bytes.to_vec();
    if out.len() >= 3 {
        out[2] = minutes_to_timer_enum(out[2]);
    }
    vec![out]
}

/// Full-payload transform for `power_inactivity_timer` write.
/// Uses the same minute → enum mapping as [`dim_timer_write_payload`].
pub fn power_timer_write_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    dim_timer_write_payload(bytes)
}

/// Converts 10 IEEE 754 big-endian float32 gain values (in dB) into 10 uint8
/// firmware values for the Arctis 7+ family: `firmware_val = clamp(round(2 ×
/// (12 + gain_dB)), 0, 48) as u8` (±12 dB range in 0.5 dB steps, vs. the Nova
/// Pro family's ±10 dB — see [`gains_to_firmware_values`]). A third device
/// with yet another offset/clamp pair should prompt generalising this instead
/// of adding a fourth near-identical function.
///
/// Input: 40 bytes (10 × f32 BE — see [`gains_to_firmware_values`]). Output:
/// 10 bytes, one per EQ band.
pub fn gains_to_firmware_values_7plus(bytes: &[u8]) -> Vec<u8> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| {
            let db = f32::from_be_bytes(*c);
            ((2.0_f32 * (12.0_f32 + db)).round() as i32).clamp(0, 48) as u8
        })
        .collect()
}

/// Full-payload transform for the Arctis 7+ `eq` write: passes `report_id`
/// and `command` through unchanged, then converts 10 × float32 gains
/// (bytes 2–41) to 10 × uint8 firmware values using
/// [`gains_to_firmware_values_7plus`].
pub fn eq_gains_7plus_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() < 2 {
        return vec![bytes.to_vec()];
    }
    let mut out = Vec::with_capacity(12);
    out.push(bytes[0]);
    out.push(bytes[1]);
    out.extend_from_slice(&gains_to_firmware_values_7plus(&bytes[2..]));
    vec![out]
}

/// Full-payload transform for `muted_mic_brightness` write (Arctis Nova 5 family).
/// Converts byte 2 from a user-facing level (0–3) to the firmware brightness
/// value (0, 1, 4, 10).  Unknown levels map to 4 (medium), matching firmware
/// behaviour on an unrecognised value.
pub fn muted_mic_brightness_write_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = bytes.to_vec();
    if out.len() >= 3 {
        out[2] = match out[2] {
            0 => 0,
            1 => 1,
            2 => 4,
            3 => 10,
            _ => 4,
        };
    }
    vec![out]
}

/// Layout of the `parametric_eq`/`_bt`/`_mic` struct as serialised by the
/// codec (see `base_arctis_nova_7_gen2.yaml`): `[report_id, eq_name_command,
/// connection_type, preset_type, eqband_command, update_complete, 10 ×
/// (frequency:u16, filter_type:u8, gain:f32, q_factor:f32), name:varstring]`.
/// The three write steps below each pick out a different slice of this same
/// serialisation and re-encode it into its own wire message — mirroring the
/// vendor spec's own `api-write` sequence (name+header, band data, commit).
const NOVA7GEN2_EQ_BAND_COUNT: usize = 10;
const NOVA7GEN2_EQ_BAND_SRC_SIZE: usize = 11; // u16 + u8 + f32 + f32
const NOVA7GEN2_EQ_BANDS_OFFSET: usize = 6;
const NOVA7GEN2_EQ_NAME_OFFSET: usize = 6 + NOVA7GEN2_EQ_BAND_COUNT * NOVA7GEN2_EQ_BAND_SRC_SIZE; // 116

/// Step 1: the name-announcement message — `[report_id, 0xA7 (eq_name_command),
/// connection_type, preset_type, name_bytes...]`.
pub fn nova7gen2_eq_name_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() < 4 {
        return vec![bytes.to_vec()];
    }
    let mut out = Vec::with_capacity(4 + bytes.len().saturating_sub(NOVA7GEN2_EQ_NAME_OFFSET));
    out.extend_from_slice(&bytes[0..4]);
    if bytes.len() > NOVA7GEN2_EQ_NAME_OFFSET {
        out.extend_from_slice(&bytes[NOVA7GEN2_EQ_NAME_OFFSET..]);
    }
    vec![out]
}

/// Step 2: the band-data message — `[report_id, 0x33 (eqband_command),
/// connection_type, 10 × (frequency:u16 BE, filter_type:u8, gain:i8,
/// q_factor:u16 LE)]`. Re-encodes each band from the engine's wire shape
/// (BE u16 frequency, u8 filter type, BE f32 gain in dB, BE f32 Q factor)
/// into the firmware's compact shape: gain becomes a single signed byte in
/// 0.1 dB units, Q factor becomes a little-endian uint16 in thousandths
/// (the vendor spec stores Q factor little-endian specifically — everything
/// else on this device is big-endian).
pub fn nova7gen2_eq_bands_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() < NOVA7GEN2_EQ_NAME_OFFSET {
        return vec![bytes.to_vec()];
    }
    let mut out = Vec::with_capacity(3 + NOVA7GEN2_EQ_BAND_COUNT * 6);
    out.push(bytes[0]);
    out.push(bytes[4]);
    out.push(bytes[2]);
    for band in 0..NOVA7GEN2_EQ_BAND_COUNT {
        let base = NOVA7GEN2_EQ_BANDS_OFFSET + band * NOVA7GEN2_EQ_BAND_SRC_SIZE;
        // frequency: passthrough, 2 bytes BE
        out.extend_from_slice(&bytes[base..base + 2]);
        // filter_type: passthrough
        out.push(bytes[base + 2]);
        // gain: f32 dB -> signed byte in 0.1 dB units
        let gain_db = f32::from_be_bytes(bytes[base + 3..base + 7].try_into().unwrap());
        let gain_byte = (gain_db * 10.0).round().clamp(-128.0, 127.0) as i8;
        out.push(gain_byte as u8);
        // q_factor: f32 -> uint16 in thousandths, little-endian on the wire
        let q = f32::from_be_bytes(bytes[base + 7..base + 11].try_into().unwrap());
        let q_u16 = (q * 1000.0).round().clamp(0.0, u16::MAX as f32) as u16;
        out.extend_from_slice(&q_u16.to_le_bytes());
    }
    vec![out]
}

/// Step 3: the commit message — `[report_id, 0x27 (update_complete)]`,
/// telling the firmware the previous two messages form a complete update.
pub fn nova7gen2_eq_commit_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() < 6 {
        return vec![bytes.to_vec()];
    }
    vec![vec![bytes[0], bytes[5]]]
}

fn minutes_to_timer_enum(minutes: u8) -> u8 {
    match minutes {
        0 => 0,
        1 => 1,
        5 => 2,
        10 => 3,
        15 => 4,
        30 => 5,
        60 => 6,
        _ => 0,
    }
}

/// Converts a row-major 1-bit packed bitmap (MSB-first, `ceil(width/8) × height`
/// bytes) into column-packed LSB-y-flipped format (`width × ceil(height/8)` bytes).
///
/// "Column-packed" = data is organised column-by-column (x outer, y inner).
/// "LSB-y-flipped" = bit 7 of each byte represents the TOP pixel of its 8-row
/// group (y = 8p), bit 0 represents the BOTTOM pixel (y = 8p + 7).  This is the
/// byte order expected by the OLED controller.
pub fn image_to_column_packed(width: usize, height: usize, bytes: &[u8]) -> Vec<u8> {
    let row_stride = width.div_ceil(8);
    let hp = height.div_ceil(8) * 8; // height padded to multiple of 8
    let col_stride = hp / 8; // bytes per output column
    let mut out = vec![0u8; width * col_stride];

    for y in 0..height {
        for x in 0..width {
            let src_byte = y * row_stride + x / 8;
            let src_bit = 7 - (x % 8); // MSB-first in row-major
            let pixel = (bytes[src_byte] >> src_bit) & 1;
            if pixel != 0 {
                let dst_byte = x * col_stride + y / 8;
                let dst_bit = 7 - (y % 8); // bit 7 = top of 8-row group (y-flipped)
                out[dst_byte] |= 1 << dst_bit;
            }
        }
    }
    out
}

/// Splits the serialised `draw_bitmap` payload into one or two HID FEATURE
/// sub-payloads as required by the Arctis Nova Pro protocol.
///
/// Input layout (from codec): `[report_id, command, x, y, width, height, bitmap…]`.
/// If `width × align(height, 8) / 8 ≤ 512` bytes: returns 1 payload covering the
/// whole bitmap.  Otherwise splits at the midpoint column (`w/2`) and returns 2
/// payloads, each with its own x/width header fields updated accordingly.
pub fn bitmap_sub_payload(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.len() < 6 {
        return vec![bytes.to_vec()];
    }
    let report_id = bytes[0];
    let command = bytes[1];
    let x = bytes[2];
    let y = bytes[3];
    let w = bytes[4] as usize;
    let h = bytes[5] as usize;
    let hp = h.div_ceil(8) * 8; // height padded to multiple of 8
    let total_size = w * hp / 8;

    if total_size <= 512 {
        let mut p = Vec::with_capacity(6 + total_size);
        p.extend_from_slice(&[report_id, command, x, y, w as u8, hp as u8]);
        p.extend_from_slice(&bytes[6..6 + total_size]);
        vec![p]
    } else {
        let w1 = w / 2;
        let w2 = w - w1;
        let s1 = w1 * hp / 8;
        let s2 = w2 * hp / 8;

        let mut p0 = Vec::with_capacity(6 + s1);
        p0.extend_from_slice(&[report_id, command, x, y, w1 as u8, hp as u8]);
        p0.extend_from_slice(&bytes[6..6 + s1]);

        let mut p1 = Vec::with_capacity(6 + s2);
        p1.extend_from_slice(&[report_id, command, x + w1 as u8, y, w2 as u8, hp as u8]);
        p1.extend_from_slice(&bytes[6 + s1..6 + s1 + s2]);

        vec![p0, p1]
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── gains_to_firmware_values ──────────────────────────────────────────────

    #[test]
    fn gains_to_firmware_values_known_values() {
        // 0 dB → 20, -10 dB → 0, +10 dB → 40, -5 dB → 10, +5 dB → 30
        let db: [f32; 10] = [0.0, -10.0, 10.0, -5.0, 5.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut input = Vec::with_capacity(40);
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(
            gains_to_firmware_values(&input),
            vec![20, 0, 40, 10, 30, 20, 20, 20, 20, 20]
        );
    }

    #[test]
    fn gains_to_firmware_values_clamps_negative() {
        // -100 dB → 2*(10-100) = -180 → clamped to 0
        let db: [f32; 10] = [-100.0; 10];
        let mut input = Vec::with_capacity(40);
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        assert!(gains_to_firmware_values(&input).iter().all(|&b| b == 0));
    }

    #[test]
    fn gains_to_firmware_values_rounding() {
        // -5.5 dB → 2*(10 - 5.5) = 9.0 → 9
        // -4.75 dB → 2*(10 - 4.75) = 10.5 → 11 (rounds to nearest even? .round() rounds away from 0)
        let db: [f32; 10] = [-5.5, -4.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut input = Vec::with_capacity(40);
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        let out = gains_to_firmware_values(&input);
        assert_eq!(out[0], 9); // round(9.0) = 9
        assert_eq!(out[1], 11); // round(10.5) = 11
    }

    // ── image_to_column_packed ────────────────────────────────────────────────

    #[test]
    fn column_packed_single_pixel_top_left() {
        // 8×8 image, only pixel (0,0) set
        // Input: row 0 = 0x80 (MSB = x=0), rows 1-7 = 0x00
        let mut input = vec![0u8; 8];
        input[0] = 0x80;
        let out = image_to_column_packed(8, 8, &input);
        // Column 0 (x=0): y=0 → bit 7 → 0x80
        assert_eq!(out[0], 0x80, "column 0 should have bit 7 set");
        assert!(out[1..].iter().all(|&b| b == 0), "all other columns zero");
    }

    #[test]
    fn column_packed_single_pixel_bottom_right() {
        // 8×8 image, only pixel (7,7) set
        // Input: rows 0-6 = 0x00, row 7 = 0x01 (LSB = x=7)
        let mut input = vec![0u8; 8];
        input[7] = 0x01;
        let out = image_to_column_packed(8, 8, &input);
        // Column 7 (x=7): y=7 → bit (7-7%8) = 0 → 0x01
        assert_eq!(out[7], 0x01, "column 7 should have bit 0 set");
        assert!(out[..7].iter().all(|&b| b == 0), "columns 0-6 zero");
    }

    #[test]
    fn column_packed_output_size_with_height_padding() {
        // 128×52 image: hp = 56 (next multiple of 8), col_stride = 7
        // Output size: 128 * 7 = 896 bytes
        // actual input: ceil(128/8)*52 = 16*52 = 832 bytes
        let input = vec![0u8; 16 * 52];
        let out = image_to_column_packed(128, 52, &input);
        assert_eq!(out.len(), 128 * 7, "128 columns × 7 bytes each");
    }

    #[test]
    fn column_packed_all_pixels_set_8x8() {
        // Every pixel set → every output byte should be 0xFF
        let input = vec![0xFF_u8; 8];
        let out = image_to_column_packed(8, 8, &input);
        assert!(out.iter().all(|&b| b == 0xFF));
    }

    // ── bitmap_sub_payload ────────────────────────────────────────────────────

    #[test]
    fn bitmap_sub_payload_single_packet_for_small_image() {
        // 128×8 → hp=8, total = 128*8/8 = 128 bytes ≤ 512 → 1 sub-payload
        let mut input = vec![0u8; 6 + 128];
        input[0] = 0x06;
        input[1] = 0x93;
        input[2] = 0; // x
        input[3] = 0; // y
        input[4] = 128; // width
        input[5] = 8; // height (hp=8)
        for i in 0..128usize {
            input[6 + i] = i as u8;
        }
        let result = bitmap_sub_payload(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(&result[0][0..6], &[0x06, 0x93, 0, 0, 128, 8]);
        assert_eq!(&result[0][6..], &input[6..6 + 128]);
    }

    #[test]
    fn bitmap_sub_payload_two_packets_for_large_image() {
        // 128×52 → hp=56, total = 128*56/8 = 896 > 512 → 2 sub-payloads
        // w1=64, s1=448; w2=64, s2=448
        let total = 896usize;
        let mut input = vec![0u8; 6 + total];
        input[0] = 0x06;
        input[1] = 0x93;
        input[2] = 0; // x
        input[3] = 12; // y
        input[4] = 128; // width
        input[5] = 52; // height
        for i in 0..total {
            input[6 + i] = (i % 251) as u8;
        }
        let result = bitmap_sub_payload(&input);
        assert_eq!(result.len(), 2);
        // First sub-payload: x=0, w=64, data[0..448]
        assert_eq!(&result[0][0..6], &[0x06, 0x93, 0, 12, 64, 56]);
        assert_eq!(&result[0][6..], &input[6..6 + 448]);
        // Second sub-payload: x=64, w=64, data[448..896]
        assert_eq!(&result[1][0..6], &[0x06, 0x93, 64, 12, 64, 56]);
        assert_eq!(&result[1][6..], &input[6 + 448..6 + 896]);
    }

    #[test]
    fn bitmap_sub_payload_exactly_512_bytes_is_single() {
        // 128×32 → hp=32, total = 128*32/8 = 512 → exactly at boundary → 1 packet
        let total = 512usize;
        let mut input = vec![0u8; 6 + total];
        input[4] = 128; // width
        input[5] = 32; // height (hp=32)
        let result = bitmap_sub_payload(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][5], 32); // hp unchanged (32 is already multiple of 8)
    }

    // ── custom_eq_gains_payload ───────────────────────────────────────────────

    #[test]
    fn custom_eq_gains_payload_preserves_header_and_converts_gains() {
        // header: report_id=0x06, command=0x33
        // gains: 0 dB × 10 → firmware 20 each
        let db = [0.0_f32; 10];
        let mut input = vec![0x06u8, 0x33];
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        let result = custom_eq_gains_payload(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], 0x06);
        assert_eq!(result[0][1], 0x33);
        assert_eq!(&result[0][2..], &[20u8; 10]);
    }

    #[test]
    fn custom_eq_gains_payload_converts_known_mix() {
        // -10 dB → 0, +10 dB → 40
        let db: [f32; 10] = [-10.0, 10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut input = vec![0x06u8, 0x33];
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        let result = custom_eq_gains_payload(&input);
        assert_eq!(result[0][2], 0); // -10 dB
        assert_eq!(result[0][3], 40); // +10 dB
        assert_eq!(&result[0][4..12], &[20u8; 8]); // 0 dB × 8
    }

    #[test]
    fn custom_eq_gains_payload_too_short_passthrough() {
        let input = vec![0x06u8];
        let result = custom_eq_gains_payload(&input);
        assert_eq!(result, vec![vec![0x06u8]]);
    }

    // ── high_gain_write_payload ───────────────────────────────────────────────

    #[test]
    fn high_gain_write_payload_maps_disabled_to_low_gain() {
        let input = vec![0x06u8, 0x27, 0]; // enabled=0
        let result = high_gain_write_payload(&input);
        assert_eq!(result[0][2], 1); // device low_gain
    }

    #[test]
    fn high_gain_write_payload_maps_enabled_to_high_gain() {
        let input = vec![0x06u8, 0x27, 1]; // enabled=1
        let result = high_gain_write_payload(&input);
        assert_eq!(result[0][2], 2); // device high_gain
    }

    #[test]
    fn high_gain_write_payload_preserves_header() {
        let input = vec![0x06u8, 0x27, 0, 0xFF];
        let result = high_gain_write_payload(&input);
        assert_eq!(result[0][0], 0x06);
        assert_eq!(result[0][1], 0x27);
        assert_eq!(result[0][3], 0xFF); // trailing bytes preserved
    }

    // ── dim_timer_write_payload / power_timer_write_payload ───────────────────

    #[test]
    fn dim_timer_write_payload_all_minute_values() {
        let cases = [(0, 0), (1, 1), (5, 2), (10, 3), (15, 4), (30, 5), (60, 6)];
        for (minutes, expected_enum) in cases {
            let input = vec![0x06u8, 0x83, minutes];
            let result = dim_timer_write_payload(&input);
            assert_eq!(
                result[0][2], expected_enum,
                "{minutes} minutes should map to enum {expected_enum}"
            );
        }
    }

    #[test]
    fn dim_timer_write_payload_unknown_minutes_maps_to_never() {
        let input = vec![0x06u8, 0x83, 45]; // not a valid minute value
        let result = dim_timer_write_payload(&input);
        assert_eq!(result[0][2], 0); // fallback to never
    }

    #[test]
    fn power_timer_write_payload_same_as_dim_timer() {
        let cases = [(0, 0), (30, 5), (60, 6)];
        for (minutes, expected_enum) in cases {
            let input = vec![0x06u8, 0xC1, minutes];
            let result = power_timer_write_payload(&input);
            assert_eq!(result[0][2], expected_enum);
        }
    }

    // ── gains_to_firmware_values_7plus / eq_gains_7plus_payload ─────────────────

    #[test]
    fn gains_to_firmware_values_7plus_known_values() {
        // 0 dB → 24, -12 dB → 0, +12 dB → 48
        let db: [f32; 10] = [0.0, -12.0, 12.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut input = Vec::with_capacity(40);
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        let out = gains_to_firmware_values_7plus(&input);
        assert_eq!(out[0], 24);
        assert_eq!(out[1], 0);
        assert_eq!(out[2], 48);
    }

    #[test]
    fn gains_to_firmware_values_7plus_clamps_out_of_range() {
        let db: [f32; 10] = [-100.0, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut input = Vec::with_capacity(40);
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        let out = gains_to_firmware_values_7plus(&input);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], 48);
    }

    #[test]
    fn eq_gains_7plus_payload_preserves_header_and_converts_gains() {
        let db = [0.0_f32; 10];
        let mut input = vec![0x00u8, 0x33];
        for &v in &db {
            input.extend_from_slice(&v.to_be_bytes());
        }
        let result = eq_gains_7plus_payload(&input);
        assert_eq!(result[0][0], 0x00);
        assert_eq!(result[0][1], 0x33);
        assert_eq!(&result[0][2..], &[24u8; 10]);
    }

    // ── muted_mic_brightness_write_payload ─────────────────────────────────────

    #[test]
    fn muted_mic_brightness_write_payload_all_levels() {
        let cases = [(0, 0), (1, 1), (2, 4), (3, 10)];
        for (level, expected_fw) in cases {
            let input = vec![0x00u8, 0xAE, level];
            let result = muted_mic_brightness_write_payload(&input);
            assert_eq!(
                result[0][2], expected_fw,
                "level {level} should map to firmware value {expected_fw}"
            );
        }
    }

    #[test]
    fn muted_mic_brightness_write_payload_unknown_level_maps_to_medium() {
        let input = vec![0x00u8, 0xAE, 9];
        let result = muted_mic_brightness_write_payload(&input);
        assert_eq!(result[0][2], 4);
    }

    #[test]
    fn muted_mic_brightness_write_payload_preserves_header() {
        let input = vec![0x00u8, 0xAE, 3];
        let result = muted_mic_brightness_write_payload(&input);
        assert_eq!(result[0][0], 0x00);
        assert_eq!(result[0][1], 0xAE);
    }

    // ── nova7gen2 parametric EQ (name / bands / commit) ─────────────────────────

    /// Builds a fake `parametric_eq` codec serialisation: 6-byte header, 10
    /// bands of `(freq: u16 BE, filter_type: u8, gain_db: f32 BE, q: f32 BE)`,
    /// then the raw name bytes.
    fn eq_input(
        connection_type: u8,
        preset_type: u8,
        bands: &[(u16, u8, f32, f32); 10],
        name: &str,
    ) -> Vec<u8> {
        let mut b = vec![0x00u8, 0xA7, connection_type, preset_type, 0x33, 0x27];
        for &(freq, filter_type, gain, q) in bands {
            b.extend_from_slice(&freq.to_be_bytes());
            b.push(filter_type);
            b.extend_from_slice(&gain.to_be_bytes());
            b.extend_from_slice(&q.to_be_bytes());
        }
        b.extend_from_slice(name.as_bytes());
        b
    }

    const FLAT_BAND: (u16, u8, f32, f32) = (0, 0, 0.0, 0.0);

    #[test]
    fn nova7gen2_eq_name_payload_includes_header_and_name() {
        let bands = [FLAT_BAND; 10];
        let input = eq_input(0x01, 1, &bands, "EQ1");
        let result = nova7gen2_eq_name_payload(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], [0x00, 0xA7, 0x01, 0x01, b'E', b'Q', b'1']);
    }

    #[test]
    fn nova7gen2_eq_name_payload_empty_name() {
        let bands = [FLAT_BAND; 10];
        let input = eq_input(0x00, 0, &bands, "");
        let result = nova7gen2_eq_name_payload(&input);
        assert_eq!(result[0], [0x00, 0xA7, 0x00, 0x00]);
    }

    #[test]
    fn nova7gen2_eq_bands_payload_converts_gain_and_q_factor() {
        let mut bands = [FLAT_BAND; 10];
        // -1.2 dB, 1.414 Q -> gain byte -12 (0xF4), q 1414 LE [0x86, 0x05]
        bands[0] = (1000, 1, -1.2, 1.414);
        // +12.0 dB, 10.0 Q -> gain byte 120 (0x78), q 10000 LE [0x10, 0x27]
        bands[1] = (20000, 6, 12.0, 10.0);
        let input = eq_input(0x02, 1, &bands, "");
        let result = nova7gen2_eq_bands_payload(&input);
        assert_eq!(result.len(), 1);
        let p = &result[0];
        assert_eq!(
            &p[0..3],
            [0x00, 0x33, 0x02],
            "report_id, 0x33, connection_type"
        );
        // band 1: freq(2) + filter_type(1) + gain(1) + q(2) = 6 bytes, starting at offset 3
        assert_eq!(&p[3..9], [0x03, 0xE8, 0x01, 0xF4, 0x86, 0x05]);
        // band 2, offset 3 + 6 = 9
        assert_eq!(&p[9..15], [0x4E, 0x20, 0x06, 0x78, 0x10, 0x27]);
        // band 3 (flat/default), offset 15
        assert_eq!(&p[15..21], [0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(p.len(), 3 + 10 * 6);
    }

    #[test]
    fn nova7gen2_eq_bands_payload_clamps_out_of_range_gain() {
        let mut bands = [FLAT_BAND; 10];
        bands[0] = (100, 1, 100.0, 0.2); // way past ±12dB — must not panic or wrap silently
        let input = eq_input(0x00, 0, &bands, "");
        let result = nova7gen2_eq_bands_payload(&input);
        assert_eq!(result[0][6], 127, "gain byte clamped to i8::MAX");
    }

    #[test]
    fn nova7gen2_eq_commit_payload_is_report_id_and_update_complete() {
        let bands = [FLAT_BAND; 10];
        let input = eq_input(0x00, 0, &bands, "ignored");
        let result = nova7gen2_eq_commit_payload(&input);
        assert_eq!(result, vec![vec![0x00, 0x27]]);
    }
}
