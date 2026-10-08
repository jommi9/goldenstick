use super::{be64, AudioInfo, Codec, Container, ProbeError, Source};
use std::io::{Read, Seek};

pub(super) fn probe<R: Read + Seek>(src: &mut Source<R>, start: u64) -> Result<AudioInfo, ProbeError> {
    let mut info = AudioInfo::new(Container::Flac, Codec::Flac);
    if start > 0 {
        info.issues.push("FLAC file has an ID3 tag in front; some players refuse these".into());
    }
    let mut pos = start + 4;
    let mut streaminfo = None;
    let mut audio_start = None;
    for _ in 0..256 {
        let h = src.read_exact_at(pos, 4)?;
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7F;
        let len = u32::from_be_bytes([0, h[1], h[2], h[3]]) as u64;
        if kind == 0 {
            if len < 34 {
                return Err(ProbeError::Corrupt("FLAC STREAMINFO block is too short".into()));
            }
            streaminfo = Some(src.read_exact_at(pos + 4, 34)?);
        }
        if kind == 127 {
            return Err(ProbeError::Corrupt("invalid FLAC metadata block".into()));
        }
        pos += 4 + len;
        if last {
            audio_start = Some(pos);
            break;
        }
        if pos >= src.len {
            break;
        }
    }
    let s = streaminfo.ok_or_else(|| ProbeError::Corrupt("FLAC file has no STREAMINFO".into()))?;
    let packed = be64(&s, 10);
    let rate = (packed >> 44) as u32;
    let channels = ((packed >> 41) & 0x7) as u16 + 1;
    let bits = ((packed >> 36) & 0x1F) as u16 + 1;
    let total = packed & 0xF_FFFF_FFFF;
    if rate == 0 {
        return Err(ProbeError::Corrupt("FLAC sample rate is zero".into()));
    }
    info.sample_rate = Some(rate);
    info.channels = Some(channels);
    info.bit_depth = Some(bits);
    match audio_start {
        None => info.issues.push("FLAC metadata never ends; file is probably truncated".into()),
        Some(a) if a >= src.len => {
            return Err(ProbeError::Corrupt("FLAC file contains no audio frames".into()));
        }
        Some(a) => {
            let frame = src.read_at_most(a, 2)?;
            if frame.len() < 2 || frame[0] != 0xFF || frame[1] & 0xFE != 0xF8 {
                info.issues.push("first FLAC audio frame has a bad sync code".into());
            }
        }
    }
    if total > 0 {
        let secs = total as f64 / rate as f64;
        info.duration_secs = Some(secs);
        if let Some(a) = audio_start {
            info.bitrate_kbps = Some(((src.len.saturating_sub(a)) as f64 * 8.0 / secs / 1000.0) as u32);
        }
    }
    Ok(info)
}
