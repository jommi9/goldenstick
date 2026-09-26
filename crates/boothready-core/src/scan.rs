//! Streaming audio scan of a mounted volume.

use crate::audio::{probe_file, AudioInfo, ProbeError};
use crate::library::{count_apple_double, walk_audio_files};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum TrackProbe {
    Ok(AudioInfo),
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannedTrack {
    /// Volume-relative, forward slashes.
    pub path: String,
    pub size: u64,
    pub probe: TrackProbe,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioScan {
    pub tracks: Vec<ScannedTrack>,
    pub apple_double: u32,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub files_done: u32,
    pub bytes_done: u64,
    pub current: String,
}

/// Probe every audio file under `root`, reporting progress as it goes so the
/// UI can stream results instead of waiting for the whole drive.
pub fn scan_audio(root: &Path, mut progress: impl FnMut(&ScanProgress, &ScannedTrack)) -> AudioScan {
    let mut scan = AudioScan::default();
    let mut p = ScanProgress { files_done: 0, bytes_done: 0, current: String::new() };
    for (rel, full, size) in walk_audio_files(root) {
        let probe = match probe_file(&full) {
            Ok(info) => TrackProbe::Ok(info),
            Err(ProbeError::Unrecognized) => TrackProbe::Unreadable("not a recognised audio file".into()),
            Err(e) => TrackProbe::Unreadable(e.to_string()),
        };
        let track = ScannedTrack { path: rel.clone(), size, probe };
        p.files_done += 1;
        p.bytes_done += size;
        p.current = rel;
        progress(&p, &track);
        scan.total_bytes += size;
        scan.tracks.push(track);
    }
    scan.apple_double = count_apple_double(root);
    scan
}
