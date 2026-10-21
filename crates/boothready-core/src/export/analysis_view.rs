//! What the window shows of one track's analysis: tempo, key, the
//! three-band waveform at a list size and a player size, and the phrases.
//!
//! The analysis is the same one an export writes to a stick, read from the
//! analysis cache: the three-band waveform comes from the `PWV7` section of
//! the `.2EX` file (low, mid and high amplitude per column, 150 columns a
//! second), the phrases from `PSSI` in the `.EXT` file and the beat times
//! they refer to from `PQTZ` in the `.DAT` file.

use super::analysis_cache::{AnalysisCache, CachedAnalysis};
use super::device_library::analyze_track;
use crate::audio::tags::MusicalKey;
use djbooth_analyzer::RhythmMode;
use serde::Serialize;
use std::path::Path;

/// Columns of the waveform drawn in a list row and in the player.
pub const PREVIEW_COLUMNS: usize = 120;
pub const OVERVIEW_COLUMNS: usize = 1000;

/// rekordbox writes `PSSI` bodies XOR-masked with this key plus the entry count.
const PSSI_MASK: [u8; 19] =
    [0xCB, 0xE1, 0xEE, 0xFA, 0xE5, 0xEE, 0xAD, 0xEE, 0xE9, 0xD2, 0xE9, 0xEB, 0xE1, 0xE9, 0xF3, 0xE8, 0xE9, 0xF4, 0xE1];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Phrase {
    pub start_secs: f32,
    /// Intro, Up, Down, Chorus, Verse, Bridge or Outro.
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalysisView {
    pub tempo_bpm: f64,
    /// Standard notation, e.g. `Am`.
    pub key: String,
    pub camelot: Option<String>,
    pub duration_secs: f64,
    /// `[low, mid, high]` amplitudes, 0 to 127.
    pub preview: Vec<[u8; 3]>,
    pub overview: Vec<[u8; 3]>,
    pub phrases: Vec<Phrase>,
    /// High, Mid or Low, the analyser's phrase mood.
    pub mood: Option<&'static str>,
}

/// The body of the first section named `fourcc` in an ANLZ file, with the
/// offset of the section's start and its header length.
fn section<'a>(file: &'a [u8], fourcc: &[u8; 4]) -> Option<(&'a [u8], usize)> {
    let mut at = u32::from_be_bytes(file.get(4..8)?.try_into().ok()?) as usize;
    while at + 12 <= file.len() {
        let header = u32::from_be_bytes(file.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let length = u32::from_be_bytes(file.get(at + 8..at + 12)?.try_into().ok()?) as usize;
        if length < 12 || at + length > file.len() {
            return None;
        }
        if &file[at..at + 4] == fourcc {
            return Some((&file[at..at + length], header));
        }
        at += length;
    }
    None
}

fn three_band(two_ex: &[u8]) -> Vec<[u8; 3]> {
    let Some((tag, header)) = section(two_ex, b"PWV7") else { return Vec::new() };
    let body = tag.get(header..).unwrap_or_default();
    body.as_chunks::<3>().0.to_vec()
}

fn downsample(columns: &[[u8; 3]], count: usize) -> Vec<[u8; 3]> {
    if columns.is_empty() {
        return Vec::new();
    }
    (0..count)
        .map(|c| {
            let start = c * columns.len() / count;
            let end = ((c + 1) * columns.len() / count).max(start + 1).min(columns.len());
            let mut out = [0u8; 3];
            for col in &columns[start..end] {
                for band in 0..3 {
                    out[band] = out[band].max(col[band]);
                }
            }
            out
        })
        .collect()
}

/// Beat times in seconds from `PQTZ`, indexed by beat number minus one.
fn beat_times(dat: &[u8]) -> Vec<f32> {
    let Some((tag, header)) = section(dat, b"PQTZ") else { return Vec::new() };
    tag.get(header..)
        .unwrap_or_default()
        .as_chunks::<8>()
        .0
        .iter()
        .map(|b| u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as f32 / 1000.0)
        .collect()
}

fn phrase_name(mood: u16, kind: u16) -> &'static str {
    match (mood, kind) {
        (_, 1) => "Intro",
        (1, 2) => "Up",
        (1, 3) => "Down",
        (1, 5) => "Chorus",
        (1, 6) => "Outro",
        (2 | 3, 8) => "Bridge",
        (2 | 3, 9) => "Chorus",
        (2 | 3, 10) => "Outro",
        _ => "Verse",
    }
}

fn phrases(ext: &[u8], beats: &[f32]) -> (Vec<Phrase>, Option<&'static str>) {
    let Some((tag, _)) = section(ext, b"PSSI") else { return (Vec::new(), None) };
    let Some(count) = tag.get(16..18).map(|b| u16::from_be_bytes([b[0], b[1]])) else { return (Vec::new(), None) };
    let mut body = tag.get(18..).unwrap_or_default().to_vec();
    if body.len() >= 2 && u16::from_be_bytes([body[0], body[1]]) > 20 {
        for (i, byte) in body.iter_mut().enumerate() {
            *byte ^= PSSI_MASK[i % PSSI_MASK.len()].wrapping_add(count as u8);
        }
    }
    let Some(mood) = body.get(0..2).map(|b| u16::from_be_bytes([b[0], b[1]])) else { return (Vec::new(), None) };
    let list = (0..count as usize)
        .filter_map(|i| {
            let entry = body.get(14 + 24 * i..14 + 24 * (i + 1))?;
            let beat = u16::from_be_bytes([entry[2], entry[3]]) as usize;
            let kind = u16::from_be_bytes([entry[4], entry[5]]);
            let start_secs = *beats.get(beat.checked_sub(1)?)?;
            Some(Phrase { start_secs, kind: phrase_name(mood, kind) })
        })
        .collect();
    let mood = match mood {
        1 => Some("High"),
        2 => Some("Mid"),
        3 => Some("Low"),
        _ => None,
    };
    (list, mood)
}

/// The window's view of one cached analysis.
pub fn view(analysis: &CachedAnalysis) -> AnalysisView {
    let [dat, ext, two_ex] = &analysis.files;
    let bands = three_band(two_ex);
    let (phrases, mood) = phrases(ext, &beat_times(dat));
    AnalysisView {
        tempo_bpm: analysis.tempo_bpm,
        key: analysis.key.clone(),
        camelot: MusicalKey::parse(&analysis.key).map(|k| k.camelot()),
        duration_secs: analysis.duration_secs,
        preview: downsample(&bands, PREVIEW_COLUMNS),
        overview: downsample(&bands, OVERVIEW_COLUMNS),
        phrases,
        mood,
    }
}

/// The cached analysis of `source`, analysing it first when `analyze` is set
/// and nothing is cached. Uses the export's steady-tempo mode, so an
/// analysis made here is reused by the next export and the other way round.
pub fn cached_or_analyzed(cache: &AnalysisCache, source: &Path, analyze: bool) -> Result<Option<AnalysisView>, String> {
    if let Some(hit) = cache.load(source, RhythmMode::Normal) {
        return Ok(Some(view(&hit)));
    }
    if !analyze {
        return Ok(None);
    }
    let fresh = analyze_track(source, RhythmMode::Normal)?;
    cache.store(source, RhythmMode::Normal, &fresh).map_err(|e| format!("keeping the analysis: {e}"))?;
    Ok(Some(view(&fresh)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_track_gives_tempo_waveforms_and_a_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("click.wav");
        std::fs::write(&wav, crate::audio::fixtures::click_track_wav(128.0, 12)).unwrap();
        let cache = AnalysisCache::new(dir.path().join("cache"));
        assert_eq!(cached_or_analyzed(&cache, &wav, false).unwrap(), None, "nothing is analysed without asking");
        let v = cached_or_analyzed(&cache, &wav, true).unwrap().expect("analysed");
        assert!((v.tempo_bpm - 128.0).abs() < 1.5, "tempo {}", v.tempo_bpm);
        assert_eq!(v.preview.len(), PREVIEW_COLUMNS);
        assert_eq!(v.overview.len(), OVERVIEW_COLUMNS);
        assert!(v.overview.iter().any(|c| c.iter().any(|&b| b > 0)), "the waveform is not flat");
        assert!((v.duration_secs - 12.0).abs() < 0.5);
        // The second call reads the cache.
        assert_eq!(cached_or_analyzed(&cache, &wav, false).unwrap(), Some(v));
    }

    #[test]
    fn sections_are_found_and_bad_lengths_stop_the_walk() {
        // A minimal file: 28-byte header, then one 16-byte PQTZ section with one beat... truncated on purpose.
        let mut f = vec![0u8; 28];
        f[0..4].copy_from_slice(b"PMAI");
        f[4..8].copy_from_slice(&28u32.to_be_bytes());
        let mut tag = b"PQTZ".to_vec();
        tag.extend_from_slice(&24u32.to_be_bytes());
        tag.extend_from_slice(&32u32.to_be_bytes());
        tag.extend_from_slice(&[0u8; 12]);
        tag.extend_from_slice(&[0, 1, 0x32, 0, 0, 0, 0x01, 0xF4]);
        f.extend_from_slice(&tag);
        assert_eq!(beat_times(&f), vec![0.5]);
        f[28 + 8..28 + 12].copy_from_slice(&999u32.to_be_bytes());
        assert!(beat_times(&f).is_empty());
    }
}
