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

    fn click_track(path: &Path, bpm: f64, seconds: u32) {
        std::fs::write(path, boothready_core::audio::fixtures::click_track_wav(bpm, seconds)).unwrap();
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
