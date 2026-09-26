use super::{le16, le32, le64, AudioInfo, Codec, Container, ProbeError, Source};
use std::io::{Read, Seek};

pub(super) fn probe<R: Read + Seek>(src: &mut Source<R>) -> Result<AudioInfo, ProbeError> {
    let page = src.read_exact_at(0, 27)?;
    let nsegs = page[26] as usize;
    let segs = src.read_exact_at(27, nsegs)?;
    let first_len: usize = segs.iter().take_while(|&&s| s == 255).count() * 255
        + segs.iter().find(|&&s| s != 255).copied().unwrap_or(0) as usize;
    let payload = src.read_exact_at(27 + nsegs as u64, first_len.min(512))?;
    let mut info;
    let rate;
    let mut pre_skip = 0u64;
    if payload.len() >= 30 && &payload[0..7] == b"\x01vorbis" {
        info = AudioInfo::new(Container::Ogg, Codec::Vorbis);
        info.channels = Some(payload[11] as u16);
        rate = le32(&payload, 12);
        let nominal = le32(&payload, 20) as i32;
        if nominal > 0 {
            info.bitrate_kbps = Some(nominal as u32 / 1000);
        }
    } else if payload.len() >= 19 && &payload[0..8] == b"OpusHead" {
        info = AudioInfo::new(Container::Ogg, Codec::Opus);
        info.channels = Some(payload[9] as u16);
        pre_skip = le16(&payload, 10) as u64;
        // Opus always decodes at 48 kHz; the header carries the source rate.
        rate = 48_000;
    } else if payload.len() >= 13 + 34 && &payload[0..5] == b"\x7FFLAC" {
        info = AudioInfo::new(Container::Ogg, Codec::Flac);
        let s = &payload[13 + 4..];
        let packed = u64::from_be_bytes(s[10..18].try_into().unwrap());
        rate = (packed >> 44) as u32;
        info.channels = Some(((packed >> 41) & 7) as u16 + 1);
        info.bit_depth = Some(((packed >> 36) & 0x1F) as u16 + 1);
    } else {
        return Err(ProbeError::Unrecognized);
    }
    if rate == 0 {
        return Err(ProbeError::Corrupt("Ogg stream declares a zero sample rate".into()));
    }
    info.sample_rate = Some(rate);

    // Duration from the granule position of the last page.
    let tail_len = src.len.min(64 * 1024);
    let tail = src.read_at_most(src.len - tail_len, tail_len as usize)?;
    if let Some(i) = tail.windows(4).rposition(|w| w == b"OggS") {
        if i + 14 <= tail.len() {
            let granule = le64(&tail, i + 6);
            if granule != u64::MAX && granule > pre_skip {
                let secs = (granule - pre_skip) as f64 / rate as f64;
                info.duration_secs = Some(secs);
                if info.bitrate_kbps.is_none() && secs > 0.0 {
                    info.bitrate_kbps = Some((src.len as f64 * 8.0 / secs / 1000.0) as u32);
                }
            }
        }
    } else {
        info.issues.push("could not find the final Ogg page; file may be truncated".into());
    }
    Ok(info)
}
