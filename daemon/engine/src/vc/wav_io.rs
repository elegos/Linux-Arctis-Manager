// Minimal, dependency-free mono WAV read/write. Not a general-purpose WAV
// library — just enough to (a) load a real recording into the [`super::inference::pipeline::Pipeline`]
// offline-verification harness and (b) later back [E10-S6b]'s calibration
// render, both of which only ever need "one real file of mono speech in,
// one real file of mono speech out". Supports 16-bit and 32-bit-float PCM
// input (stereo is downmixed by averaging channels); always writes 16-bit
// PCM output, since that's the WAV flavour every media player handles
// without surprises.

use std::io::{Read, Write};
use std::path::Path;

/// Read a WAV file's audio as mono `f32` in `[-1, 1]`, plus its native
/// sample rate. Multi-channel input is downmixed by averaging channels.
pub fn read_mono_f32(path: &Path) -> std::io::Result<(Vec<f32>, u32)> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;

    if buf.len() < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a RIFF/WAVE file",
        ));
    }

    let mut channels = 1u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 16u16;
    let mut is_float = false;
    let mut data: &[u8] = &[];

    let mut pos = 12usize;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let size = u32::from_le_bytes(buf[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(buf.len());
        match id {
            b"fmt " => {
                let fmt = &buf[body_start..body_end];
                let format_tag = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                bits_per_sample = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
                is_float = format_tag == 3; // WAVE_FORMAT_IEEE_FLOAT
            }
            b"data" => {
                data = &buf[body_start..body_end];
            }
            _ => {}
        }
        // Chunks are word-aligned: a trailing pad byte follows an odd-sized body.
        pos = body_start + size + (size & 1);
    }

    if sample_rate == 0 || data.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "WAV file missing fmt/data chunks",
        ));
    }

    let channels = channels.max(1) as usize;
    let frame_bytes = (bits_per_sample as usize / 8) * channels;
    let n_frames = data.len() / frame_bytes.max(1);

    let mut mono = Vec::with_capacity(n_frames);
    for frame in data.chunks_exact(frame_bytes) {
        let mut sum = 0.0f32;
        for ch in 0..channels {
            let s = &frame[ch * (bits_per_sample as usize / 8)..];
            let v = match (bits_per_sample, is_float) {
                (16, false) => i16::from_le_bytes(s[0..2].try_into().unwrap()) as f32 / 32768.0,
                (32, true) => f32::from_le_bytes(s[0..4].try_into().unwrap()),
                (32, false) => {
                    i32::from_le_bytes(s[0..4].try_into().unwrap()) as f32 / 2147483648.0
                }
                (8, false) => (s[0] as f32 - 128.0) / 128.0,
                (bits, float) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported WAV sample format: {bits}-bit float={float}"),
                    ))
                }
            };
            sum += v;
        }
        mono.push(sum / channels as f32);
    }

    Ok((mono, sample_rate))
}

/// Write mono `f32` samples (clamped to `[-1, 1]`) as a 16-bit PCM WAV file.
pub fn write_mono_pcm16(path: &Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let data_bytes = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let i = (clamped * 32767.0).round() as i16;
        out.extend_from_slice(&i.to_le_bytes());
    }

    std::fs::File::create(path)?.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_16bit_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        write_mono_pcm16(&path, &samples, 48000).unwrap();
        let (read_back, sr) = read_mono_f32(&path).unwrap();
        assert_eq!(sr, 48000);
        assert_eq!(read_back.len(), samples.len());
        for (a, b) in samples.iter().zip(read_back.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn stereo_is_downmixed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        // Hand-roll a tiny stereo 16-bit PCM WAV: left=1.0, right=-1.0 for
        // one frame -> mono downmix should read back as ~0.0.
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36u32 + 4).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes()); // stereo
        out.extend_from_slice(&48000u32.to_le_bytes());
        out.extend_from_slice(&(48000u32 * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&32767i16.to_le_bytes());
        out.extend_from_slice(&(-32768i16).to_le_bytes());
        std::fs::write(&path, &out).unwrap();

        let (mono, sr) = read_mono_f32(&path).unwrap();
        assert_eq!(sr, 48000);
        assert_eq!(mono.len(), 1);
        assert!(mono[0].abs() < 1e-3, "expected ~0.0, got {}", mono[0]);
    }
}
