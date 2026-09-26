//! Hand-built audio files for tests. Each builder produces the smallest
//! structurally valid file for its format, so tests run without any external
//! encoder. The ffmpeg-based tests in `tests.rs` cross-check real encoders.

use super::aiff::f64_to_extended;
use super::mp3::test_header;

pub fn wav(rate: u32, bits: u16, channels: u16, frames: u32, extensible: bool) -> Vec<u8> {
    let block = channels as u32 * bits as u32 / 8;
    let data_len = frames * block;
    let fmt_len: u32 = if extensible { 40 } else { 16 };
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(4 + 8 + fmt_len + 8 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&fmt_len.to_le_bytes());
    v.extend_from_slice(&(if extensible { 0xFFFEu16 } else { 1 }).to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * block).to_le_bytes());
    v.extend_from_slice(&(block as u16).to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    if extensible {
        v.extend_from_slice(&22u16.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(&3u32.to_le_bytes());
        // KSDATAFORMAT_SUBTYPE_PCM
        v.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    }
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.resize(v.len() + data_len as usize, 0);
    v
}

pub fn aiff(rate: u32, bits: u16, channels: u16, frames: u32, sowt: bool) -> Vec<u8> {
    let block = channels as u32 * bits as u32 / 8;
    let data_len = frames * block;
    let comm_len: u32 = if sowt { 18 + 4 + 2 } else { 18 };
    let mut v = Vec::new();
    v.extend_from_slice(b"FORM");
    v.extend_from_slice(&(4 + 8 + comm_len + 8 + 8 + data_len).to_be_bytes());
    v.extend_from_slice(if sowt { b"AIFC" } else { b"AIFF" });
    v.extend_from_slice(b"COMM");
    v.extend_from_slice(&comm_len.to_be_bytes());
    v.extend_from_slice(&channels.to_be_bytes());
    v.extend_from_slice(&frames.to_be_bytes());
    v.extend_from_slice(&bits.to_be_bytes());
    v.extend_from_slice(&f64_to_extended(rate));
    if sowt {
        v.extend_from_slice(b"sowt");
        v.extend_from_slice(&[0, 0]); // empty pascal string, padded
    }
    v.extend_from_slice(b"SSND");
    v.extend_from_slice(&(8 + data_len).to_be_bytes());
    v.extend_from_slice(&[0; 8]);
    v.resize(v.len() + data_len as usize, 0);
    v
}

pub fn flac(rate: u32, bits: u16, channels: u16, total_samples: u64) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"fLaC");
    v.push(0x80); // last block, STREAMINFO
    v.extend_from_slice(&[0, 0, 34]);
    v.extend_from_slice(&4096u16.to_be_bytes());
    v.extend_from_slice(&4096u16.to_be_bytes());
    v.extend_from_slice(&[0; 6]);
    let packed: u64 = ((rate as u64) << 44)
        | (((channels - 1) as u64) << 41)
        | (((bits - 1) as u64) << 36)
        | (total_samples & 0xF_FFFF_FFFF);
    v.extend_from_slice(&packed.to_be_bytes());
    v.extend_from_slice(&[0; 16]);
    v.extend_from_slice(&[0xFF, 0xF8, 0x69, 0x08]);
    v.resize(v.len() + 1000, 0);
    v
}

/// CBR MPEG-1 Layer III at 128 kbps / 44.1 kHz with an ID3v2 tag in front.
pub fn mp3_cbr(frames: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"ID3\x04\x00\x00");
    v.extend_from_slice(&[0, 0, 0, 20]);
    v.resize(v.len() + 20, 0);
    let h = test_header(9, 0, false); // 128 kbps, 44.1 kHz
    let frame_len = 144 * 128_000 / 44_100; // 417, no padding
    for _ in 0..frames {
        v.extend_from_slice(&h.to_be_bytes());
        v.resize(v.len() + frame_len - 4, 0);
    }
    v
}

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn full_atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend_from_slice(body);
    atom(kind, &b)
}

/// Minimal M4A with one audio track. `codec` is "aac-lc", "he-aac" or "alac".
pub fn m4a(codec: &str, rate: u32, seconds: u32) -> Vec<u8> {
    let timescale = rate;
    let mut mdhd = Vec::new();
    mdhd.extend_from_slice(&[0; 8]);
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&(timescale * seconds).to_be_bytes());
    mdhd.extend_from_slice(&[0; 4]);
    let mut hdlr = vec![0u8; 4];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0; 13]);

    let mut entry = Vec::new();
    entry.extend_from_slice(&[0; 6]);
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0; 8]); // version, revision, vendor
    entry.extend_from_slice(&2u16.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes());
    entry.extend_from_slice(&[0; 4]);
    entry.extend_from_slice(&(rate << 16).to_be_bytes());
    let fourcc: &[u8; 4] = if codec == "alac" {
        let mut c = Vec::new();
        c.extend_from_slice(&4096u32.to_be_bytes());
        c.extend_from_slice(&[0, 24, 40, 10, 14, 2]);
        c.extend_from_slice(&255u16.to_be_bytes());
        c.extend_from_slice(&0u32.to_be_bytes());
        c.extend_from_slice(&2_000_000u32.to_be_bytes());
        c.extend_from_slice(&rate.to_be_bytes());
        entry.extend_from_slice(&full_atom(b"alac", &c));
        b"alac"
    } else {
        let aot: u8 = if codec == "he-aac" { 5 } else { 2 };
        let freq_idx: u8 = match rate {
            48000 => 3,
            _ => 4,
        };
        let asc = [(aot << 3) | (freq_idx >> 1), ((freq_idx & 1) << 7) | (2 << 3)];
        let mut dsi = vec![0x05, asc.len() as u8];
        dsi.extend_from_slice(&asc);
        let mut dcd = vec![0x40, 0x15, 0, 0, 0];
        dcd.extend_from_slice(&320_000u32.to_be_bytes());
        dcd.extend_from_slice(&256_000u32.to_be_bytes());
        dcd.extend_from_slice(&dsi);
        let mut dcd_full = vec![0x04, dcd.len() as u8];
        dcd_full.extend_from_slice(&dcd);
        let mut es = vec![0, 1, 0];
        es.extend_from_slice(&dcd_full);
        let mut es_full = vec![0x03, es.len() as u8];
        es_full.extend_from_slice(&es);
        entry.extend_from_slice(&full_atom(b"esds", &es_full));
        b"mp4a"
    };
    let sample_entry = atom(fourcc, &entry);
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&sample_entry);
    let stbl = atom(b"stbl", &full_atom(b"stsd", &stsd));
    let minf = atom(b"minf", &stbl);
    let mut mdia_body = full_atom(b"mdhd", &mdhd);
    mdia_body.extend_from_slice(&full_atom(b"hdlr", &hdlr));
    mdia_body.extend_from_slice(&minf);
    let trak = atom(b"trak", &atom(b"mdia", &mdia_body));
    let moov = atom(b"moov", &trak);
    let mut v = atom(b"ftyp", b"M4A \x00\x00\x00\x00M4A mp42isom");
    v.extend_from_slice(&atom(b"mdat", &vec![0u8; 4000]));
    v.extend_from_slice(&moov);
    v
}
