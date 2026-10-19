//! A cache of analysis results, so a track analysed for one export is not
//! analysed again for the next.
//!
//! Analysing a track takes seconds while copying it takes a moment, so a
//! second export of the same playlists would otherwise cost as much as the
//! first. Each entry holds the analyser's three ANLZ files as generated,
//! before their path tag is pointed at the exported media, plus the tempo,
//! key and length the export reads from them. Entries are keyed by the
//! file's resolved path, size, modification time and the rhythm mode, so an
//! edited or replaced file is analysed afresh.
//!
//! The cache holds no file names in clear, only hashes, and lives in the
//! app's own data folder.

use djbooth_analyzer::RhythmMode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const ENTRY_VERSION: u32 = 1;
const FILES: [&str; 3] = ["ANLZ0000.DAT", "ANLZ0000.EXT", "ANLZ0000.2EX"];

/// One track's analysis as the analyser produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedAnalysis {
    /// DAT, EXT and 2EX bytes, in that order, with the analyser's own path tag.
    pub files: [Vec<u8>; 3],
    pub tempo_bpm: f64,
    pub key: String,
    pub duration_secs: f64,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    version: u32,
    tempo_bpm: f64,
    key: String,
    duration_secs: f64,
    stored_unix: u64,
}

/// A folder of cached analyses.
#[derive(Debug, Clone)]
pub struct AnalysisCache {
    dir: PathBuf,
}

impl AnalysisCache {
    pub fn new(dir: impl Into<PathBuf>) -> AnalysisCache {
        AnalysisCache { dir: dir.into() }
    }

    /// The entry name for a file, or `None` when the file cannot be read.
    pub fn key(source: &Path, mode: RhythmMode) -> Option<String> {
        let meta = fs::metadata(source).ok()?;
        if !meta.is_file() {
            return None;
        }
        let resolved = source.canonicalize().ok()?;
        let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
        let mut hasher = blake3::Hasher::new();
        hasher.update(resolved.to_string_lossy().as_bytes());
        hasher.update(b"\n");
        hasher.update(&meta.len().to_le_bytes());
        hasher.update(&modified.to_le_bytes());
        hasher.update(mode_name(mode).as_bytes());
        Some(hasher.finalize().to_hex().to_string())
    }

    fn entry_dir(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    /// Whether an analysis of `source` in `mode` is cached.
    pub fn has(&self, source: &Path, mode: RhythmMode) -> bool {
        Self::key(source, mode).is_some_and(|key| {
            let dir = self.entry_dir(&key);
            dir.join("meta.json").is_file() && FILES.iter().all(|f| dir.join(f).is_file())
        })
    }

    /// The cached analysis of `source` in `mode`, if there is one.
    pub fn load(&self, source: &Path, mode: RhythmMode) -> Option<CachedAnalysis> {
        let dir = self.entry_dir(&Self::key(source, mode)?);
        let meta: Meta = serde_json::from_slice(&fs::read(dir.join("meta.json")).ok()?).ok()?;
        if meta.version != ENTRY_VERSION {
            return None;
        }
        let [dat, ext, two_ex] = FILES.map(|f| fs::read(dir.join(f)).ok());
        Some(CachedAnalysis {
            files: [dat?, ext?, two_ex?],
            tempo_bpm: meta.tempo_bpm,
            key: meta.key,
            duration_secs: meta.duration_secs,
        })
    }

    /// Keep an analysis of `source`. Failures are returned, never fatal: an
    /// export goes on without the cache.
    pub fn store(&self, source: &Path, mode: RhythmMode, analysis: &CachedAnalysis) -> std::io::Result<()> {
        let key = Self::key(source, mode)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "the source file can't be read"))?;
        fs::create_dir_all(&self.dir)?;
        // Built next to its final name and renamed into place, so a reader
        // never sees a half-written entry.
        let staging = self.dir.join(format!(".{key}.{}", std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir(&staging)?;
        for (name, bytes) in FILES.iter().zip(&analysis.files) {
            fs::write(staging.join(name), bytes)?;
        }
        let meta = Meta {
            version: ENTRY_VERSION,
            tempo_bpm: analysis.tempo_bpm,
            key: analysis.key.clone(),
            duration_secs: analysis.duration_secs,
            stored_unix: crate::manifest::now_unix(),
        };
        fs::write(staging.join("meta.json"), serde_json::to_vec(&meta).map_err(std::io::Error::other)?)?;
        let target = self.entry_dir(&key);
        let _ = fs::remove_dir_all(&target);
        fs::rename(&staging, &target)
    }
}

fn mode_name(mode: RhythmMode) -> &'static str {
    match mode {
        RhythmMode::Normal => "normal",
        RhythmMode::Dynamic => "dynamic",
        RhythmMode::Auto => "auto",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_follow_the_file_and_the_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let track = tmp.path().join("a.wav");
        fs::write(&track, b"audio").unwrap();
        let cache = AnalysisCache::new(tmp.path().join("cache"));
        assert!(!cache.has(&track, RhythmMode::Normal));
        assert!(cache.load(&track, RhythmMode::Normal).is_none());
        let analysis = CachedAnalysis {
            files: [b"PMAI1".to_vec(), b"PMAI2".to_vec(), b"PMAI3".to_vec()],
            tempo_bpm: 124.5,
            key: "Am".into(),
            duration_secs: 301.25,
        };
        cache.store(&track, RhythmMode::Normal, &analysis).unwrap();
        assert!(cache.has(&track, RhythmMode::Normal));
        assert!(!cache.has(&track, RhythmMode::Dynamic), "the rhythm mode is part of the key");
        assert_eq!(cache.load(&track, RhythmMode::Normal).unwrap(), analysis);
        assert!(fs::read_dir(tmp.path().join("cache")).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("a.wav")));

        // A changed file is a different entry.
        fs::write(&track, b"audio, longer").unwrap();
        assert!(!cache.has(&track, RhythmMode::Normal));
        assert!(cache.store(&tmp.path().join("missing.wav"), RhythmMode::Normal, &analysis).is_err());
        assert!(!cache.has(tmp.path(), RhythmMode::Normal), "a folder has no entry");
    }
}
