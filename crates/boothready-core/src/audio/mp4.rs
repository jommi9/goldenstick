use super::{arr, be16, be32, be64, AudioInfo, Codec, Container, ProbeError, Source};
use std::io::{Read, Seek};

const MAX_DEPTH: u32 = 10;
const MAX_ATOMS: u32 = 20_000;
/// Largest atom body we'll pull into memory (the `stsd` sample description).
const MAX_BODY: u64 = 64 * 1024;

#[derive(Default)]
struct Found {
    has_moov: bool,
    audio_tracks: u32,
    video_tracks: u32,
    timescale: u32,
    duration: u64,
    format: Option<[u8; 4]>,
    channels: Option<u16>,
    sample_size: Option<u16>,
    sample_rate: Option<u32>,
    alac_bits: Option<u16>,
    alac_rate: Option<u32>,
    object_type: Option<u8>,
    audio_object_type: Option<u8>,
    avg_bitrate: Option<u32>,
    atoms: u32,
    in_sound_track: bool,
}

pub(super) fn probe<R: Read + Seek>(src: &mut Source<R>) -> Result<AudioInfo, ProbeError> {
    let mut f = Found::default();
    walk(src, 0, src.len, 0, &mut f)?;
    if !f.has_moov {
        return Err(ProbeError::Corrupt("MP4 file has no 'moov' index (incomplete download or copy?)".into()));
    }
    let fmt = f.format.ok_or_else(|| {
        if f.video_tracks > 0 {
            ProbeError::Corrupt("MP4 file contains video but no audio track".into())
        } else {
            ProbeError::Corrupt("MP4 file has no audio track".into())
        }
    })?;
    let codec = match &fmt {
        b"alac" => Codec::Alac,
        b"fLaC" => Codec::Flac,
        b"Opus" => Codec::Opus,
        b"drms" | b"enca" | b"drmi" => Codec::Protected,
        b"mp4a" => match f.object_type {
            Some(0x40) | Some(0x66) | Some(0x67) | Some(0x68) => match f.audio_object_type {
                Some(2) | None => Codec::AacLc,
                Some(5) | Some(29) => Codec::AacHe,
                Some(_) => Codec::AacOther,
            },
            Some(0x69) | Some(0x6B) => Codec::Mp3,
            _ => Codec::Other,
        },
        _ => Codec::Other,
    };
    let mut info = AudioInfo::new(Container::Mp4, codec);
    info.channels = f.channels;
    info.sample_rate = f.alac_rate.or(f.sample_rate).filter(|&r| r > 0);
    info.bit_depth = match codec {
        Codec::Alac => f.alac_bits.or(f.sample_size),
        Codec::Flac => f.sample_size,
        _ => None,
    };
    if f.timescale > 0 && f.duration > 0 {
        let secs = f.duration as f64 / f.timescale as f64;
        info.duration_secs = Some(secs);
        info.bitrate_kbps = f
            .avg_bitrate
            .filter(|&b| b > 0)
            .map(|b| b / 1000)
            .or_else(|| Some((src.len as f64 * 8.0 / secs / 1000.0) as u32));
    }
    if f.video_tracks > 0 {
        info.issues.push("file also contains a video track".into());
    }
    if f.audio_tracks > 1 {
        info.issues.push(format!("{} audio tracks; players use the first", f.audio_tracks));
    }
    Ok(info)
}

fn walk<R: Read + Seek>(
    src: &mut Source<R>,
    start: u64,
    end: u64,
    depth: u32,
    f: &mut Found,
) -> Result<(), ProbeError> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let mut pos = start;
    while pos + 8 <= end {
        f.atoms += 1;
        if f.atoms > MAX_ATOMS {
            return Err(ProbeError::Corrupt("MP4 structure is unreasonably large".into()));
        }
        let h = src.read_exact_at(pos, 8)?;
        let mut size = be32(&h, 0) as u64;
        let kind: [u8; 4] = arr(&h, 4);
        let mut header = 8u64;
        if size == 1 {
            let ext = src.read_exact_at(pos + 8, 8)?;
            size = be64(&ext, 0);
            header = 16;
        } else if size == 0 {
            size = end - pos;
        }
        if size < header || pos + size > end {
            if depth == 0 && &kind == b"mdat" {
                // Truncated media data; the index may still be intact.
                break;
            }
            return Err(ProbeError::Corrupt(format!(
                "MP4 atom '{}' has an invalid size",
                String::from_utf8_lossy(&kind)
            )));
        }
        let body = pos + header;
        let body_end = pos + size;
        match &kind {
            b"moov" => {
                f.has_moov = true;
                walk(src, body, body_end, depth + 1, f)?;
            }
            b"trak" => {
                f.in_sound_track = false;
                walk(src, body, body_end, depth + 1, f)?;
            }
            b"mdia" | b"minf" | b"stbl" => walk(src, body, body_end, depth + 1, f)?,
            b"hdlr" => {
                let b = src.read_exact_at(body, 12)?;
                match &b[8..12] {
                    b"soun" => {
                        f.audio_tracks += 1;
                        f.in_sound_track = true;
                    }
                    b"vide" => f.video_tracks += 1,
                    _ => {}
                }
            }
            b"mdhd" => {
                let b = src.read_exact_at(body, 32.min(size - header) as usize)?;
                // mdhd precedes hdlr inside mdia, so remember it for whichever
                // track turns out to be the first audio one.
                if f.format.is_none() {
                    if b[0] == 1 && b.len() >= 32 {
                        f.timescale = be32(&b, 20);
                        f.duration = be64(&b, 24);
                    } else if b.len() >= 20 {
                        f.timescale = be32(&b, 12);
                        f.duration = be32(&b, 16) as u64;
                    }
                }
            }
            b"stsd" if f.in_sound_track && f.format.is_none() => {
                let len = (size - header).min(MAX_BODY) as usize;
                let b = src.read_exact_at(body, len)?;
                parse_stsd(&b, f);
            }
            _ => {}
        }
        pos = body_end;
    }
    Ok(())
}

fn parse_stsd(b: &[u8], f: &mut Found) {
    // version/flags (4), entry count (4), first sample entry.
    if b.len() < 8 + 36 {
        return;
    }
    let e = &b[8..];
    let entry_size = (be32(e, 0) as usize).min(e.len());
    let fmt: [u8; 4] = arr(e, 4);
    f.format = Some(fmt);
    // SampleEntry: reserved(6) + data_reference_index(2); AudioSampleEntry
    // then has version(2) revision(2) vendor(4) channels(2) samplesize(2)
    // compression(2) packetsize(2) samplerate(4, 16.16).
    f.channels = Some(be16(e, 24));
    f.sample_size = Some(be16(e, 26));
    f.sample_rate = Some(be32(e, 32) >> 16);
    let version = be16(e, 16);
    let children_start = match version {
        1 => 36 + 16,
        2 => 36 + 36,
        _ => 36,
    };
    if version == 2 && e.len() >= 36 + 36 {
        // QuickTime v2 sound description stores rate as a float64.
        let rate = f64::from_bits(be64(e, 40));
        if rate.is_finite() && rate > 0.0 && rate < 1e7 {
            f.sample_rate = Some(rate.round() as u32);
        }
        f.channels = Some(be32(e, 48) as u16);
    }
    let mut p = children_start;
    let end = entry_size;
    let mut guard = 0;
    while p + 8 <= end && guard < 64 {
        guard += 1;
        let size = be32(e, p) as usize;
        if size < 8 || p + size > end {
            break;
        }
        let kind = &e[p + 4..p + 8];
        let body = &e[p + 8..p + size];
        match kind {
            b"alac" if body.len() >= 4 + 24 => {
                let c = &body[4..];
                f.alac_bits = Some(c[5] as u16);
                f.channels = Some(c[9] as u16);
                f.avg_bitrate = Some(be32(c, 16));
                f.alac_rate = Some(be32(c, 20));
            }
            b"esds" if body.len() > 4 => parse_esds(&body[4..], f),
            b"wave" => {
                // QuickTime wraps esds inside 'wave'; look one level down.
                let mut q = 0;
                while q + 8 <= body.len() {
                    let s = be32(body, q) as usize;
                    if s < 8 || q + s > body.len() {
                        break;
                    }
                    if &body[q + 4..q + 8] == b"esds" && s > 12 {
                        parse_esds(&body[q + 12..q + s], f);
                    }
                    q += s;
                }
            }
            _ => {}
        }
        p += size;
    }
}

/// Parse an MPEG-4 ES descriptor far enough to find the object type and the
/// AAC audio object type.
fn parse_esds(d: &[u8], f: &mut Found) {
    fn read_len(d: &[u8], p: &mut usize) -> Option<usize> {
        let mut len = 0usize;
        for _ in 0..4 {
            let b = *d.get(*p)?;
            *p += 1;
            len = (len << 7) | (b & 0x7F) as usize;
            if b & 0x80 == 0 {
                break;
            }
        }
        Some(len)
    }
    let mut p = 0usize;
    let Some(&tag) = d.get(p) else { return };
    p += 1;
    if tag != 0x03 {
        return;
    }
    if read_len(d, &mut p).is_none() || p + 3 > d.len() {
        return;
    }
    let flags = d[p + 2];
    p += 3;
    if flags & 0x80 != 0 {
        p += 2;
    }
    if flags & 0x40 != 0 {
        let Some(&l) = d.get(p) else { return };
        p += 1 + l as usize;
    }
    if flags & 0x20 != 0 {
        p += 2;
    }
    if d.get(p) != Some(&0x04) {
        return;
    }
    p += 1;
    if read_len(d, &mut p).is_none() || p + 13 > d.len() {
        return;
    }
    f.object_type = Some(d[p]);
    f.avg_bitrate = Some(be32(d, p + 9));
    p += 13;
    if d.get(p) != Some(&0x05) {
        return;
    }
    p += 1;
    if read_len(d, &mut p).is_none() || p >= d.len() {
        return;
    }
    let mut aot = d[p] >> 3;
    if aot == 31 && p + 1 < d.len() {
        aot = 32 + (((d[p] & 0x07) << 3) | (d[p + 1] >> 5));
    }
    f.audio_object_type = Some(aot);
}
