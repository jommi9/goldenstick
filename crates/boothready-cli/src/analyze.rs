//! `boothready analyze`: beat grid, key, phrase and waveform analysis for one
//! track, written as the ANLZ files (DAT, EXT, 2EX) Pioneer players read.
//!
//! The analysis itself is djbooth's clean-room analyser
//! (`crates/djbooth-analyzer`). This command only reads the audio file and
//! writes into the output folder it is given; it never touches a device.

use anyhow::{bail, Context, Result};
use djbooth_analyzer::{analyze_to_directory_with_options, AnalysisOptions, AnalyzerOutput, RhythmMode};
use serde::Serialize;
use std::path::Path;

/// Parse the `--rhythm-mode` value.
pub fn rhythm_mode(value: &str) -> Result<RhythmMode> {
    match value {
        "normal" => Ok(RhythmMode::Normal),
        "dynamic" => Ok(RhythmMode::Dynamic),
        "auto" => Ok(RhythmMode::Auto),
        other => bail!("unknown rhythm mode '{other}': use normal, dynamic or auto"),
    }
}

/// What `--json` prints: the numbers a person or a script needs, plus the files written.
#[derive(Serialize)]
pub struct AnalysisReport {
    pub audio: String,
    pub tempo_bpm: f64,
    pub key: String,
    pub beats: usize,
    pub first_downbeat_ms: u32,
    pub phrases: usize,
    pub rhythm_mode: String,
    pub files: Vec<(String, u64)>,
}

impl From<&AnalyzerOutput> for AnalysisReport {
    fn from(o: &AnalyzerOutput) -> Self {
        Self {
            audio: o.audio_path.clone(),
            tempo_bpm: o.summary.tempo_bpm,
            key: o.summary.key.clone(),
            beats: o.summary.beat_grid.beat_count,
            first_downbeat_ms: o.summary.beat_grid.first_downbeat_ms,
            phrases: o.summary.song_structure.phrase_count,
            rhythm_mode: o.rhythm_mode.clone(),
            files: o.outputs.clone(),
        }
    }
}

/// Analyse `audio` and write its ANLZ files into `out`.
pub fn analyze(audio: &Path, out: &Path, mode: RhythmMode) -> Result<AnalysisReport> {
    if !audio.is_file() {
        bail!("{} is not an audio file", audio.display());
    }
    if out.exists() && !out.is_dir() {
        bail!("{} exists and is not a folder", out.display());
    }
    let options = AnalysisOptions::automatic().with_rhythm_mode(mode);
    let output = analyze_to_directory_with_options(audio, out, None, options)
        .with_context(|| format!("analysing {}", audio.display()))?;
    Ok(AnalysisReport::from(&output))
}

/// One line for the terminal.
pub fn describe(r: &AnalysisReport, out: &Path) -> String {
    format!(
        "{}: {:.2} BPM, key {}, {} beats, {} phrases. {} analysis files in {}",
        Path::new(&r.audio).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        r.tempo_bpm,
        r.key,
        r.beats,
        r.phrases,
        r.files.len(),
        out.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A 16-bit stereo WAV of short clicks at `bpm`, `seconds` long.
    fn click_track(path: &Path, bpm: f64, seconds: u32) {
        let rate = 44_100u32;
        let frames = rate * seconds;
        let beat = (f64::from(rate) * 60.0 / bpm) as u32;
        let mut pcm = Vec::with_capacity(frames as usize * 4);
        for i in 0..frames {
            let pos = i % beat;
            let s = if pos < 400 {
                let env = 1.0 - f64::from(pos) / 400.0;
                (f64::from(pos) * 2.0 * std::f64::consts::PI * 1000.0 / f64::from(rate)).sin() * env * 0.8
            } else {
                0.0
            };
            let v = (s * f64::from(i16::MAX)) as i16;
            pcm.extend_from_slice(&v.to_le_bytes());
            pcm.extend_from_slice(&v.to_le_bytes());
        }
        let mut f = std::fs::File::create(path).unwrap();
        let data = pcm.len() as u32;
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&rate.to_le_bytes()).unwrap();
        f.write_all(&(rate * 4).to_le_bytes()).unwrap();
        f.write_all(&4u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data.to_le_bytes()).unwrap();
        f.write_all(&pcm).unwrap();
    }

    #[test]
    fn rhythm_modes_parse() {
        assert!(matches!(rhythm_mode("normal").unwrap(), RhythmMode::Normal));
        assert!(matches!(rhythm_mode("auto").unwrap(), RhythmMode::Auto));
        assert!(rhythm_mode("fast").is_err());
    }

    #[test]
    fn click_track_gets_its_tempo_and_three_anlz_files() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("clicks.wav");
        click_track(&audio, 128.0, 30);
        let out = dir.path().join("anlz");
        let report = analyze(&audio, &out, RhythmMode::Normal).unwrap();
        assert!((report.tempo_bpm - 128.0).abs() < 0.6, "tempo {}", report.tempo_bpm);
        for ext in ["DAT", "EXT", "2EX"] {
            assert!(out.join(format!("ANLZ0000.{ext}")).is_file(), "missing ANLZ0000.{ext}");
        }
    }

    #[test]
    fn refuses_a_file_as_the_output_folder() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("clicks.wav");
        click_track(&audio, 120.0, 4);
        let out = dir.path().join("not-a-folder");
        std::fs::write(&out, b"x").unwrap();
        assert!(analyze(&audio, &out, RhythmMode::Normal).is_err());
    }
}
