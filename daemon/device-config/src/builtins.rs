/// Converts 10 IEEE 754 little-endian float32 gain values (in dB) into 10 uint8
/// firmware values: `firmware_val = clamp(round(2 × (10 + gain_dB)), 0, 255) as u8`.
///
/// Input: 40 bytes (10 × f32 LE).  Output: 10 bytes, one per EQ band.
pub fn gains_to_firmware_values(bytes: &[u8]) -> Vec<u8> {
    bytes
        .chunks_exact(4)
        .map(|c| {
            let db = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
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
            input.extend_from_slice(&v.to_le_bytes());
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
            input.extend_from_slice(&v.to_le_bytes());
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
            input.extend_from_slice(&v.to_le_bytes());
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
            input.extend_from_slice(&v.to_le_bytes());
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
            input.extend_from_slice(&v.to_le_bytes());
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
}
