use super::{be16, be32, AudioInfo, Codec, Container, ProbeError, Source};
use std::io::{Read, Seek};

pub(super) fn probe<R: Read + Seek>(src: &mut Source<R>) -> Result<AudioInfo, ProbeError> {
    let head = src.read_exact_at(0, 12)?;
    let aifc = &head[8..12] == b"AIFC";
    let form_size = be32(&head, 4) as u64;
    let mut info = AudioInfo::new(Container::Aiff, Codec::Pcm);
    if form_size + 8 > src.len {
        info.issues.push("file is shorter than its AIFF header says (truncated copy?)".into());
    }
    let mut pos = 12u64;
    let mut frames: Option<u32> = None;
    let mut have_comm = false;
    let mut have_ssnd = false;
    for _ in 0..512 {
        if pos + 8 > src.len {
            break;
        }
        let ch = src.read_exact_at(pos, 8)?;
        let size = be32(&ch, 4) as u64;
        let body = pos + 8;
        match &ch[0..4] {
            b"COMM" => {
                if size < 18 {
                    return Err(ProbeError::Corrupt("AIFF COMM chunk is too short".into()));
                }
                let c = src.read_exact_at(body, size.min(26) as usize)?;
                info.channels = Some(be16(&c, 0));
                frames = Some(be32(&c, 2));
                info.bit_depth = Some(be16(&c, 6));
                let rate = extended_to_f64(&c[8..18]);
                if !(rate.is_finite() && (1.0..=1_000_000.0).contains(&rate)) {
                    return Err(ProbeError::Corrupt("AIFF sample rate is invalid".into()));
                }
                info.sample_rate = Some(rate.round() as u32);
                if aifc && c.len() >= 22 {
                    info.codec = match &c[18..22] {
                        b"NONE" | b"sowt" | b"twos" | b"in24" | b"in32" => Codec::Pcm,
                        b"fl32" | b"FL32" | b"fl64" | b"FL64" => Codec::PcmFloat,
                        _ => Codec::Other,
                    };
                    if &c[18..22] == b"sowt" {
                        info.issues.push("little-endian AIFF-C ('sowt'); some players reject it".into());
                    }
                }
                have_comm = true;
            }
            b"SSND" => {
                have_ssnd = true;
                if body + size > src.len {
                    info.issues.push("audio data is cut short".into());
                }
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    if !have_comm {
        return Err(ProbeError::Corrupt("AIFF file has no COMM chunk".into()));
    }
    if !have_ssnd && frames.unwrap_or(0) > 0 {
        return Err(ProbeError::Corrupt("AIFF file has no sound data chunk".into()));
    }
    if let (Some(f), Some(r)) = (frames, info.sample_rate) {
        info.duration_secs = Some(f as f64 / r as f64);
        if let (Some(ch), Some(bits)) = (info.channels, info.bit_depth) {
            info.bitrate_kbps = Some((r as u64 * ch as u64 * bits as u64 / 1000) as u32);
        }
    }
    Ok(info)
}

/// IEEE 754 80-bit extended precision, big-endian, as used by AIFF.
pub(crate) fn extended_to_f64(b: &[u8]) -> f64 {
    let sign = if b[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = (((b[0] & 0x7F) as i32) << 8) | b[1] as i32;
    let mant = u64::from_be_bytes(b[2..10].try_into().unwrap());
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    if exp == 0x7FFF {
        return f64::NAN;
    }
    sign * (mant as f64) * 2f64.powi(exp - 16383 - 63)
}

/// Inverse of [`extended_to_f64`] for positive integers; used by fixtures.
#[cfg(test)]
pub(crate) fn f64_to_extended(v: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if v == 0 {
        return out;
    }
    let shift = v.leading_zeros();
    let mant = (v as u64) << (32 + shift);
    let exp = 16383 + 31 - shift as i32;
    out[0] = (exp >> 8) as u8;
    out[1] = exp as u8;
    out[2..10].copy_from_slice(&mant.to_be_bytes());
    out
}
