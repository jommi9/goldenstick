use super::{be32, AudioInfo, Codec, Container, ProbeError, Source};
use std::io::{Read, Seek};

/// Total length of any ID3v2 tag(s) at the start of the file.
pub(super) fn id3v2_len<R: Read + Seek>(src: &mut Source<R>) -> Result<u64, ProbeError> {
    let mut pos = 0u64;
    // Files occasionally carry more than one stacked tag.
    for _ in 0..4 {
        let h = src.read_at_most(pos, 10)?;
        if h.len() < 10 || &h[0..3] != b"ID3" {
            break;
        }
        if h[6..10].iter().any(|&b| b & 0x80 != 0) {
            return Err(ProbeError::Corrupt("ID3v2 tag has an invalid size".into()));
        }
        let size = ((h[6] as u64) << 21) | ((h[7] as u64) << 14) | ((h[8] as u64) << 7) | h[9] as u64;
        let footer = if h[5] & 0x10 != 0 { 10 } else { 0 };
        pos += 10 + size + footer;
    }
    Ok(pos)
}

#[derive(Debug, Clone, Copy)]
struct FrameHeader {
    version: u8, // 1 = MPEG-1, 2 = MPEG-2, 25 = MPEG-2.5
    layer: u8,
    bitrate_kbps: u32,
    sample_rate: u32,
    padding: bool,
    mono: bool,
}

impl FrameHeader {
    fn parse(h: u32) -> Option<FrameHeader> {
        if h & 0xFFE0_0000 != 0xFFE0_0000 {
            return None;
        }
        let version = match (h >> 19) & 3 {
            0 => 25,
            2 => 2,
            3 => 1,
            _ => return None,
        };
        let layer = match (h >> 17) & 3 {
            1 => 3,
            2 => 2,
            3 => 1,
            _ => return None,
        };
        let br_idx = ((h >> 12) & 0xF) as usize;
        let sr_idx = ((h >> 10) & 3) as usize;
        if br_idx == 0 || br_idx == 15 || sr_idx == 3 {
            // Free-format bitrate is legal but vanishingly rare; treat as no sync.
            return None;
        }
        const V1_L1: [u32; 15] = [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448];
        const V1_L2: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384];
        const V1_L3: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
        const V2_L1: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256];
        const V2_L23: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
        let bitrate_kbps = match (version, layer) {
            (1, 1) => V1_L1[br_idx],
            (1, 2) => V1_L2[br_idx],
            (1, 3) => V1_L3[br_idx],
            (_, 1) => V2_L1[br_idx],
            _ => V2_L23[br_idx],
        };
        let base = [44100, 48000, 32000][sr_idx];
        let sample_rate = match version {
            1 => base,
            2 => base / 2,
            _ => base / 4,
        };
        Some(FrameHeader {
            version,
            layer,
            bitrate_kbps,
            sample_rate,
            padding: (h >> 9) & 1 == 1,
            mono: (h >> 6) & 3 == 3,
        })
    }

    fn samples_per_frame(&self) -> u32 {
        match (self.layer, self.version) {
            (1, _) => 384,
            (2, _) => 1152,
            (3, 1) => 1152,
            _ => 576,
        }
    }

    fn frame_len(&self) -> u64 {
        let pad = self.padding as u64;
        if self.layer == 1 {
            (12 * self.bitrate_kbps as u64 * 1000 / self.sample_rate as u64 + pad) * 4
        } else {
            self.samples_per_frame() as u64 / 8 * self.bitrate_kbps as u64 * 1000 / self.sample_rate as u64 + pad
        }
    }

    /// Offset of the Xing/Info header from the frame start (after side info).
    fn xing_offset(&self) -> usize {
        4 + match (self.version, self.mono) {
            (1, false) => 32,
            (1, true) => 17,
            (_, false) => 17,
            (_, true) => 9,
        }
    }
}

pub(super) fn probe<R: Read + Seek>(src: &mut Source<R>) -> Result<AudioInfo, ProbeError> {
    let start = id3v2_len(src)?;
    if start >= src.len {
        return Err(ProbeError::Corrupt("file contains only a tag and no audio".into()));
    }
    // Search for two consecutive valid frame headers so random 0xFFE bytes
    // inside album art don't fool us.
    const WINDOW: usize = 256 * 1024;
    let buf = src.read_at_most(start, WINDOW)?;
    let mut found = None;
    let mut i = 0usize;
    while i + 4 <= buf.len() {
        if buf[i] == 0xFF && buf[i + 1] & 0xE0 == 0xE0 {
            if let Some(h) = FrameHeader::parse(be32(&buf, i)) {
                let next = i as u64 + h.frame_len();
                let next_ok = match src.read_at_most(start + next, 4)? {
                    n if n.len() == 4 => FrameHeader::parse(be32(&n, 0))
                        .map(|h2| h2.version == h.version && h2.layer == h.layer && h2.sample_rate == h.sample_rate)
                        .unwrap_or(false),
                    // A single frame right at end of file still counts.
                    _ => start + next >= src.len,
                };
                if next_ok {
                    found = Some((i, h));
                    break;
                }
            }
        }
        i += 1;
    }
    let (off, h) = found.ok_or_else(|| ProbeError::Corrupt("no valid MPEG audio frames found".into()))?;
    let frame_pos = start + off as u64;
    let codec = if h.layer == 3 { Codec::Mp3 } else { Codec::Mp2 };
    let mut info = AudioInfo::new(Container::Mp3, codec);
    info.sample_rate = Some(h.sample_rate);
    info.channels = Some(if h.mono { 1 } else { 2 });
    info.bitrate_kbps = Some(h.bitrate_kbps);
    if off > 0 {
        info.issues.push(format!("{off} bytes of junk before the first audio frame"));
    }
    if h.version != 1 {
        info.issues.push(format!(
            "MPEG-{} audio at {} Hz; many players only support MPEG-1",
            if h.version == 2 { "2" } else { "2.5" },
            h.sample_rate
        ));
    }

    // VBR headers carry a frame count, which gives an exact duration.
    let first = src.read_at_most(frame_pos, 200)?;
    let xo = h.xing_offset();
    let mut frames: Option<u32> = None;
    if first.len() >= xo + 12 && (&first[xo..xo + 4] == b"Xing" || &first[xo..xo + 4] == b"Info") {
        let flags = be32(&first, xo + 4);
        if flags & 1 != 0 {
            frames = Some(be32(&first, xo + 8));
        }
        info.vbr = &first[xo..xo + 4] == b"Xing";
    } else if first.len() >= 36 + 18 && &first[36..40] == b"VBRI" {
        frames = Some(be32(&first, 36 + 14));
        info.vbr = true;
    }
    let audio_bytes = src.len - frame_pos;
    match frames {
        Some(n) if n > 0 => {
            let secs = n as f64 * h.samples_per_frame() as f64 / h.sample_rate as f64;
            info.duration_secs = Some(secs);
            if info.vbr && secs > 0.0 {
                info.bitrate_kbps = Some((audio_bytes as f64 * 8.0 / secs / 1000.0).round() as u32);
            }
        }
        _ => {
            info.duration_secs = Some(audio_bytes as f64 * 8.0 / (h.bitrate_kbps as f64 * 1000.0));
        }
    }
    Ok(info)
}

/// Build a valid MPEG-1 Layer III frame header, for fixtures.
#[cfg(any(test, feature = "fixtures"))]
pub(crate) fn test_header(bitrate_idx: u32, sr_idx: u32, mono: bool) -> u32 {
    0xFFFB_0000 | (bitrate_idx << 12) | (sr_idx << 10) | if mono { 3 << 6 } else { 0 }
}
