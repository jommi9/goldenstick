//! Audio file probing.
//!
//! These are header-only parsers written for untrusted input: every size read
//! from a file is bounds-checked against the real file length, chunk walks
//! have iteration limits, and nothing allocates more than a small fixed
//! buffer. We deliberately avoid decoding audio; compatibility depends on the
//! container, codec, sample rate and bit depth, which the headers carry.

mod aiff;
mod flac;
mod mp3;
mod mp4;
mod ogg;
mod wav;

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Wav,
    Aiff,
    Flac,
    Mp3,
    Mp4,
    Ogg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    /// Integer PCM (WAV, AIFF, AIFF-C "sowt"/"NONE").
    Pcm,
    /// IEEE float PCM.
    PcmFloat,
    Mp3,
    /// MPEG-1/2 Layer I or II.
    Mp2,
    /// AAC Low Complexity.
    AacLc,
    /// HE-AAC (SBR) or HE-AACv2 (SBR+PS).
    AacHe,
    /// Any other AAC object type.
    AacOther,
    Alac,
    Flac,
    Vorbis,
    Opus,
    /// FairPlay or other encrypted track (e.g. `.m4p`). Unplayable anywhere
    /// outside Apple's own software.
    Protected,
    Other,
}

impl Codec {
    pub fn label(self) -> &'static str {
        match self {
            Codec::Pcm => "PCM",
            Codec::PcmFloat => "32-bit float PCM",
            Codec::Mp3 => "MP3",
            Codec::Mp2 => "MPEG Layer I/II",
            Codec::AacLc => "AAC",
            Codec::AacHe => "HE-AAC",
            Codec::AacOther => "AAC (uncommon profile)",
            Codec::Alac => "ALAC",
            Codec::Flac => "FLAC",
            Codec::Vorbis => "Ogg Vorbis",
            Codec::Opus => "Opus",
            Codec::Protected => "DRM protected",
            Codec::Other => "Unknown codec",
        }
    }
}

/// What a header tells us about one audio file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioInfo {
    pub container: Container,
    pub codec: Codec,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u16>,
    pub channels: Option<u16>,
    pub bitrate_kbps: Option<u32>,
    pub duration_secs: Option<f64>,
    /// WAV using WAVE_FORMAT_EXTENSIBLE. Some older players reject it.
    #[serde(default)]
    pub wav_extensible: bool,
    /// Variable bitrate MP3 (Xing/VBRI header present).
    #[serde(default)]
    pub vbr: bool,
    /// Non-fatal integrity problems (truncation, odd headers).
    pub issues: Vec<String>,
}

impl AudioInfo {
    pub(crate) fn new(container: Container, codec: Codec) -> Self {
        AudioInfo {
            container,
            codec,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate_kbps: None,
            duration_secs: None,
            wav_extensible: false,
            vbr: false,
            issues: vec![],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("could not read file: {0}")]
    Io(#[from] io::Error),
    #[error("not a recognised audio file")]
    Unrecognized,
    #[error("damaged or unreadable: {0}")]
    Corrupt(String),
}

/// File extensions the scanner treats as audio.
pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "m4a", "aac", "mp4", "m4p", "alac", "wav", "wave", "aif", "aiff", "aifc", "flac", "ogg", "oga", "opus"];

pub fn is_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Probe a file on disk.
pub fn probe_file(path: &Path) -> Result<AudioInfo, ProbeError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut src = Source::new(BufReader::with_capacity(64 * 1024, file), len);
    probe(&mut src)
}

/// Probe any seekable stream.
pub fn probe_reader<R: Read + Seek>(reader: R, len: u64) -> Result<AudioInfo, ProbeError> {
    let mut src = Source::new(reader, len);
    probe(&mut src)
}

fn probe<R: Read + Seek>(src: &mut Source<R>) -> Result<AudioInfo, ProbeError> {
    if src.len == 0 {
        return Err(ProbeError::Corrupt("file is empty".into()));
    }
    let head = src.read_at_most(0, 12)?;
    if head.len() >= 12 {
        match (&head[0..4], &head[8..12]) {
            (b"RIFF", b"WAVE") | (b"RF64", b"WAVE") => return wav::probe(src),
            (b"FORM", b"AIFF") | (b"FORM", b"AIFC") => return aiff::probe(src),
            _ => {}
        }
        if &head[4..8] == b"ftyp" {
            return mp4::probe(src);
        }
    }
    if head.starts_with(b"fLaC") {
        return flac::probe(src, 0);
    }
    if head.starts_with(b"OggS") {
        return ogg::probe(src);
    }
    if head.starts_with(b"ID3") {
        // FLAC files sometimes carry an ID3v2 tag in front.
        let after = mp3::id3v2_len(src)?;
        if src.read_at_most(after, 4)?.as_slice() == b"fLaC" {
            return flac::probe(src, after);
        }
        return mp3::probe(src);
    }
    if head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 {
        return mp3::probe(src);
    }
    // Some MP4s start with a 'wide' or 'free' atom before 'ftyp'.
    if head.len() >= 8 && matches!(&head[4..8], b"wide" | b"free" | b"skip" | b"moov" | b"mdat") {
        return mp4::probe(src);
    }
    Err(ProbeError::Unrecognized)
}

/// Bounded random-access reader over a stream of known length.
pub(crate) struct Source<R> {
    inner: R,
    pub(crate) len: u64,
}

impl<R: Read + Seek> Source<R> {
    fn new(inner: R, len: u64) -> Self {
        Source { inner, len }
    }

    /// Read up to `n` bytes at `off`; fewer if the file ends first.
    pub(crate) fn read_at_most(&mut self, off: u64, n: usize) -> io::Result<Vec<u8>> {
        if off >= self.len {
            return Ok(Vec::new());
        }
        let n = (n as u64).min(self.len - off) as usize;
        let mut buf = vec![0u8; n];
        self.inner.seek(SeekFrom::Start(off))?;
        let mut filled = 0;
        while filled < n {
            match self.inner.read(&mut buf[filled..])? {
                0 => break,
                k => filled += k,
            }
        }
        buf.truncate(filled);
        Ok(buf)
    }

    /// Read exactly `n` bytes at `off` or report the file as truncated.
    pub(crate) fn read_exact_at(&mut self, off: u64, n: usize) -> Result<Vec<u8>, ProbeError> {
        let v = self.read_at_most(off, n)?;
        if v.len() < n {
            return Err(ProbeError::Corrupt(format!("file ends unexpectedly at byte {}", off + v.len() as u64)));
        }
        Ok(v)
    }
}

pub(crate) fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}
pub(crate) fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn be64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}
pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
#[cfg(test)]
mod tests;
