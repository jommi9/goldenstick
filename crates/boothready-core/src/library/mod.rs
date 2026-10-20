//! Detection and validation of DJ libraries on a mounted volume.
//!
//! Integration levels follow the PRD: rekordbox Device Library is read and
//! validated (level 1-2), OneLibrary is detected with heuristics only (level
//! 0) because the database is encrypted, Engine DJ databases are read and
//! validated, Serato crates are detected.

mod engine;
#[cfg(feature = "sqlite")]
pub mod onelibrary;
#[cfg(feature = "onelibrary-writer")]
pub mod onelibrary_writer;
pub mod pdb;
#[cfg(any(test, feature = "fixtures"))]
pub mod pdb_fixture;
#[cfg(feature = "device-library-writer")]
pub mod pdb_writer;
mod resolve;

use crate::audio::{is_audio_path, AudioInfo};
pub use engine::EngineReport;
use pdb::{parse_pdb, MAX_PDB_BYTES};
pub use resolve::PathResolver;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Library database formats DJ hardware consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryFormat {
    /// `PIONEER/rekordbox/export.pdb`: CDJ-3000 and earlier.
    RekordboxDeviceLibrary,
    /// `PIONEER/rekordbox/exportLibrary.db`: CDJ-3000X, XDJ-AZ, OPUS-QUAD, OMNIS-DUO.
    RekordboxOneLibrary,
    /// `Engine Library/Database2/m.db`: Engine DJ 2.x and later.
    EngineDatabase,
    /// `Engine Library/m.db`: Engine Prime 1.x.
    EnginePrimeLegacy,
    /// `_Serato_/`: Serato crates.
    SeratoLibrary,
}

impl LibraryFormat {
    pub fn label(self) -> &'static str {
        match self {
            LibraryFormat::RekordboxDeviceLibrary => "rekordbox Device Library",
            LibraryFormat::RekordboxOneLibrary => "rekordbox OneLibrary",
            LibraryFormat::EngineDatabase => "Engine DJ library",
            LibraryFormat::EnginePrimeLegacy => "Engine Prime (1.x) library",
            LibraryFormat::SeratoLibrary => "Serato crates",
        }
    }
}

pub const REKORDBOX_DIR: &str = "PIONEER/rekordbox";
pub const DEVICE_LIBRARY_FILE: &str = "PIONEER/rekordbox/export.pdb";
pub const DEVICE_LIBRARY_EXT_FILE: &str = "PIONEER/rekordbox/exportExt.pdb";
/// OneLibrary (formerly "Device Library Plus"). Confirm against a current
/// rekordbox 7.2.x export before relying on the exact name in rules.
pub const ONE_LIBRARY_FILE: &str = "PIONEER/rekordbox/exportLibrary.db";
pub const ONE_LIBRARY_WAL_FILE: &str = "PIONEER/rekordbox/exportLibrary.db-wal";
pub const ANALYSIS_DIR: &str = "PIONEER/USBANLZ";
pub const ENGINE_DB_FILE: &str = "Engine Library/Database2/m.db";
pub const ENGINE_LEGACY_DB_FILE: &str = "Engine Library/m.db";
pub const SERATO_DIR: &str = "_Serato_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub is_folder: bool,
    pub track_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryTrack {
    pub id: u32,
    pub title: String,
    pub artist: Option<String>,
    /// Path relative to the volume root, forward slashes, no leading slash.
    pub path: String,
    pub db_sample_rate: Option<u32>,
    pub db_bit_depth: Option<u16>,
    pub db_file_size: Option<u64>,
    #[serde(default)]
    pub db_duration_secs: Option<u16>,
    #[serde(default)]
    pub db_bitrate_kbps: Option<u32>,
}

/// What a size difference between a file and the export's record means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeChange {
    /// The audio matches what rekordbox recorded, so only the tags changed.
    /// Serato, for one, stores its cue points and beatgrid in the tags.
    TagsOnly,
    /// The length, bitrate or sample rate differs, or could not be compared,
    /// so the file was probably replaced after the export.
    Replaced,
}

/// Compare a mismatched file's audio with the export's record of it.
///
/// rekordbox stores each track's length, bitrate and sample rate. When the
/// file on the drive still has the same length and sample rate, and the same
/// bitrate where the bitrate is constant, its audio is the one rekordbox
/// analysed and only the tags around it changed.
pub fn classify_size_change(track: &LibraryTrack, info: Option<&AudioInfo>) -> SizeChange {
    let (Some(info), Some(recorded)) = (info, track.db_duration_secs) else { return SizeChange::Replaced };
    let Some(duration) = info.duration_secs else { return SizeChange::Replaced };
    let same_length = (duration - f64::from(recorded)).abs() <= 1.5;
    let same_rate = match (info.sample_rate, track.db_sample_rate) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    let same_bitrate = match (info.bitrate_kbps, track.db_bitrate_kbps) {
        (Some(a), Some(b)) if !info.vbr => a.abs_diff(b) <= 2,
        _ => true,
    };
    if same_length && same_rate && same_bitrate {
        SizeChange::TagsOnly
    } else {
        SizeChange::Replaced
    }
}

/// Every size mismatch on a drive, classified against the scanned audio.
pub fn size_changes(dl: &DeviceLibraryReport, audio: &crate::scan::AudioScan) -> Vec<(String, SizeChange)> {
    use std::collections::HashMap;
    let probes: HashMap<&str, &AudioInfo> = audio
        .tracks
        .iter()
        .filter_map(|t| match &t.probe {
            crate::scan::TrackProbe::Ok(info) => Some((t.path.as_str(), info)),
            crate::scan::TrackProbe::Unreadable(_) => None,
        })
        .collect();
    let tracks: HashMap<&str, &LibraryTrack> = dl.tracks.iter().map(|t| (t.path.as_str(), t)).collect();
    dl.size_mismatch
        .iter()
        .map(|path| {
            let kind = match tracks.get(path.as_str()) {
                Some(track) => {
                    // The scan resolves case-insensitively; look up both spellings.
                    let info = probes
                        .get(path.as_str())
                        .or_else(|| probes.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|(_, i)| i));
                    classify_size_change(track, info.copied())
                }
                None => SizeChange::Replaced,
            };
            (path.clone(), kind)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLibraryReport {
    pub modified_unix: Option<u64>,
    pub size_bytes: u64,
    pub has_ext: bool,
    pub tracks: Vec<LibraryTrack>,
    pub playlists: Vec<PlaylistSummary>,
    /// Tracks whose audio file is not on the volume.
    pub missing: Vec<String>,
    /// Tracks whose file size differs from what the export recorded.
    pub size_mismatch: Vec<String>,
    /// The mismatched files whose tags hold Serato data.
    #[serde(default)]
    pub serato_tagged: Vec<String>,
    pub parse_error: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OneLibraryReport {
    pub modified_unix: Option<u64>,
    pub size_bytes: u64,
    /// OneLibrary databases are SQLCipher-encrypted; a plain SQLite header
    /// means something other than rekordbox wrote this file.
    pub encrypted: bool,
    /// Tracks and playlists read from inside the database with rekordbox's
    /// published key, or `None` when it was not read (no SQLite support in
    /// this build, or the key did not fit).
    #[serde(default)]
    pub tracks: Option<u32>,
    #[serde(default)]
    pub playlists: Option<u32>,
}

/// One rekordbox database was last written well before the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryLag {
    pub older: LibraryFormat,
    pub newer: LibraryFormat,
    pub seconds: u64,
}

impl LibraryLag {
    pub fn describe(&self) -> String {
        format!(
            "The {} was last written {} before the {}, so it may be missing newer tracks and playlists.",
            self.older.label(),
            rough_span(self.seconds),
            self.newer.label()
        )
    }
}

fn rough_span(seconds: u64) -> String {
    let (n, unit) = match seconds {
        s if s < 2 * 86_400 => (s / 3600, "hour"),
        s if s < 60 * 86_400 => (s / 86_400, "day"),
        s if s < 730 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    format!("about {n} {unit}{}", if n == 1 { "" } else { "s" })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RekordboxReport {
    pub device_library: Option<DeviceLibraryReport>,
    pub one_library: Option<OneLibraryReport>,
    /// Set when the two databases were written more than an hour apart.
    #[serde(default)]
    pub lag: Option<LibraryLag>,
    pub analysis_files: u32,
    pub has_settings: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeratoReport {
    pub has_database: bool,
    pub crates: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryReport {
    pub formats: Vec<LibraryFormat>,
    pub rekordbox: Option<RekordboxReport>,
    pub engine: Option<EngineReport>,
    pub serato: Option<SeratoReport>,
}

impl LibraryReport {
    pub fn has(&self, f: LibraryFormat) -> bool {
        self.formats.contains(&f)
    }

    /// Paths of every audio file some library on the drive refers to.
    pub fn referenced_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if let Some(dl) = self.rekordbox.as_ref().and_then(|r| r.device_library.as_ref()) {
            v.extend(dl.tracks.iter().map(|t| t.path.clone()));
        }
        if let Some(e) = &self.engine {
            v.extend(e.track_paths.iter().cloned());
        }
        v.sort();
        v.dedup();
        v
    }

    /// Any library-level problem that makes the drive unsafe to rely on.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if let Some(rb) = &self.rekordbox {
            if let Some(dl) = &rb.device_library {
                if let Some(e) = &dl.parse_error {
                    p.push(format!("rekordbox Device Library is damaged: {e}"));
                }
                if !dl.missing.is_empty() {
                    p.push(format!("{} tracks in the rekordbox export are missing from the drive", dl.missing.len()));
                }
                // Size differences are described by the drive summary, which
                // can tell retagged files from replaced ones.
            }
            p.extend(rb.lag.iter().map(LibraryLag::describe));
            p.extend(rb.warnings.iter().cloned());
        }
        if let Some(e) = &self.engine {
            if let Some(err) = &e.error {
                p.push(format!("Engine DJ library could not be read: {err}"));
            }
            if !e.missing.is_empty() {
                p.push(format!("{} tracks in the Engine DJ library are missing from the drive", e.missing.len()));
            }
        }
        p
    }
}

fn modified_unix(meta: &fs::Metadata) -> Option<u64> {
    meta.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn newest_modified(resolver: &mut PathResolver, files: &[&str]) -> Option<u64> {
    files.iter().filter_map(|rel| existing(resolver, rel)).filter_map(|p| modified_unix(&fs::metadata(p).ok()?)).max()
}

/// When the Device Library was last written. rekordbox stamps export.pdb
/// itself with a fixed date (1 January 2024 on a real 2026 export), so the
/// time comes from whichever of it and exportExt.pdb is newer.
pub fn device_library_written_unix(resolver: &mut PathResolver) -> Option<u64> {
    newest_modified(resolver, &[DEVICE_LIBRARY_FILE, DEVICE_LIBRARY_EXT_FILE])
}

/// When OneLibrary was last written, counting changes still in its SQLite WAL.
pub fn one_library_written_unix(resolver: &mut PathResolver) -> Option<u64> {
    newest_modified(resolver, &[ONE_LIBRARY_FILE, ONE_LIBRARY_WAL_FILE])
}

/// Resolve a volume-relative path, tolerating case differences.
fn existing(resolver: &mut PathResolver, rel: &str) -> Option<PathBuf> {
    resolver.resolve(rel)
}

/// Inspect every DJ library we know about under `root`. Never writes.
pub fn scan_libraries(root: &Path) -> LibraryReport {
    let mut resolver = PathResolver::new(root);
    let mut formats = Vec::new();
    let rekordbox = scan_rekordbox(&mut resolver);
    if let Some(rb) = &rekordbox {
        if rb.device_library.is_some() {
            formats.push(LibraryFormat::RekordboxDeviceLibrary);
        }
        if rb.one_library.is_some() {
            formats.push(LibraryFormat::RekordboxOneLibrary);
        }
    }
    let engine = engine::scan(&mut resolver);
    if let Some(e) = &engine {
        formats.push(if e.legacy { LibraryFormat::EnginePrimeLegacy } else { LibraryFormat::EngineDatabase });
    }
    let serato = existing(&mut resolver, SERATO_DIR).filter(|p| p.is_dir()).map(|dir| {
        let crates = fs::read_dir(dir.join("Subcrates"))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("crate")))
                    .count() as u32
            })
            .unwrap_or(0);
        SeratoReport { has_database: dir.join("database V2").exists(), crates }
    });
    if serato.is_some() {
        formats.push(LibraryFormat::SeratoLibrary);
    }
    LibraryReport { formats, rekordbox, engine, serato }
}

/// Count the tracks and playlists in a OneLibrary database without touching
/// the drive: the database and any `-wal` file are copied to a temporary
/// folder first, because SQLite may create `-shm` and journal files beside
/// a database it opens, and a check never writes to the stick.
#[cfg(feature = "sqlite")]
fn read_one_library_copy(db: &Path) -> Option<(u32, u32)> {
    let dir = std::env::temp_dir().join(format!("boothready-onelibrary-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&dir).ok()?;
    let result = (|| {
        let copy = dir.join("exportLibrary.db");
        fs::copy(db, &copy).ok()?;
        let wal = db.with_file_name("exportLibrary.db-wal");
        if wal.exists() {
            fs::copy(&wal, dir.join("exportLibrary.db-wal")).ok()?;
        }
        let lib = onelibrary::read_one_library_file(&copy).ok()?;
        let playlists = lib.playlists.iter().filter(|p| !p.is_folder).count();
        Some((lib.tracks.len() as u32, playlists as u32))
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}

#[cfg(not(feature = "sqlite"))]
fn read_one_library_copy(_db: &Path) -> Option<(u32, u32)> {
    None
}

fn scan_rekordbox(resolver: &mut PathResolver) -> Option<RekordboxReport> {
    let rb_dir = existing(resolver, REKORDBOX_DIR)?;
    if !rb_dir.is_dir() {
        return None;
    }
    let mut warnings = Vec::new();
    let device_library = existing(resolver, DEVICE_LIBRARY_FILE).map(|p| read_device_library(&p, resolver));
    let one_library_written = one_library_written_unix(resolver);
    let one_library = existing(resolver, ONE_LIBRARY_FILE).and_then(|p| {
        let meta = fs::metadata(&p).ok()?;
        let mut head = [0u8; 16];
        let n = fs::File::open(&p).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
        let encrypted = n == 16 && &head != b"SQLite format 3\0";
        let contents = if encrypted && meta.len() > 0 { read_one_library_copy(&p) } else { None };
        Some(OneLibraryReport {
            modified_unix: one_library_written,
            size_bytes: meta.len(),
            encrypted,
            tracks: contents.as_ref().map(|c| c.0),
            playlists: contents.as_ref().map(|c| c.1),
        })
    });
    if let Some(ol) = &one_library {
        if ol.size_bytes == 0 {
            warnings.push("rekordbox OneLibrary database is empty (export interrupted?)".into());
        } else if ol.encrypted && ol.tracks.is_none() && cfg!(feature = "sqlite") {
            warnings.push("rekordbox OneLibrary database did not open with rekordbox's published key".into());
        }
        if let (Some(ol_tracks), Some(dl)) = (ol.tracks, &device_library) {
            let dl_tracks = dl.tracks.len() as u32;
            if dl.parse_error.is_none() && ol_tracks != dl_tracks {
                warnings.push(format!(
                    "rekordbox OneLibrary lists {ol_tracks} tracks and the Device Library {dl_tracks}; newer players read only OneLibrary"
                ));
            }
        }
    }
    let mut lag = None;
    if let (Some(dl), Some(ol)) = (&device_library, &one_library) {
        if let (Some(a), Some(b)) = (dl.modified_unix, ol.modified_unix) {
            // rekordbox writes both databases during one export. A large gap
            // suggests the older one was left over from an earlier export.
            if a.abs_diff(b) > 3600 {
                let (older, newer) = if a < b {
                    (LibraryFormat::RekordboxDeviceLibrary, LibraryFormat::RekordboxOneLibrary)
                } else {
                    (LibraryFormat::RekordboxOneLibrary, LibraryFormat::RekordboxDeviceLibrary)
                };
                lag = Some(LibraryLag { older, newer, seconds: a.abs_diff(b) });
            }
        }
    }
    if device_library.is_none() && one_library.is_none() {
        warnings.push("A PIONEER/rekordbox folder exists but holds no library database".into());
    }
    let analysis_files = existing(resolver, ANALYSIS_DIR)
        .map(|d| {
            walkdir::WalkDir::new(d)
                .max_depth(4)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .count() as u32
        })
        .unwrap_or(0);
    let has_settings = existing(resolver, "PIONEER/MYSETTING.DAT").is_some();
    Some(RekordboxReport { device_library, one_library, lag, analysis_files, has_settings, warnings })
}

/// True when the leading ID3v2 tag of a file holds Serato data. Reads at
/// most the tag, capped at 4 MiB.
fn has_serato_tags(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = fs::File::open(path) else { return false };
    let mut head = [0u8; 10];
    if file.read_exact(&mut head).is_err() || &head[..3] != b"ID3" {
        return false;
    }
    let size = head[6..10].iter().fold(0usize, |n, b| (n << 7) | usize::from(b & 0x7f));
    let mut tag = vec![0u8; size.min(4 << 20)];
    let n = file.read(&mut tag).unwrap_or(0);
    tag[..n].windows(6).any(|w| w == b"Serato")
}

fn read_device_library(path: &Path, resolver: &mut PathResolver) -> DeviceLibraryReport {
    let meta = fs::metadata(path).ok();
    let mut report = DeviceLibraryReport {
        modified_unix: device_library_written_unix(resolver),
        size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        has_ext: existing(resolver, DEVICE_LIBRARY_EXT_FILE).is_some(),
        tracks: vec![],
        playlists: vec![],
        missing: vec![],
        size_mismatch: vec![],
        serato_tagged: vec![],
        parse_error: None,
        warnings: vec![],
    };
    if report.size_bytes > MAX_PDB_BYTES {
        report.parse_error = Some("export.pdb is implausibly large".into());
        return report;
    }
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            report.parse_error = Some(format!("could not read export.pdb: {e}"));
            return report;
        }
    };
    let pdb = match parse_pdb(&data) {
        Ok(p) => p,
        Err(e) => {
            report.parse_error = Some(e.to_string());
            return report;
        }
    };
    report.warnings = pdb.warnings;
    for t in pdb.tracks {
        let rel = t.file_path.trim_start_matches('/').to_string();
        match existing(resolver, &rel).and_then(|p| fs::metadata(p).ok()) {
            None => report.missing.push(rel.clone()),
            Some(m) => {
                // export.pdb stores size as u32; compare modulo 2^32.
                if t.file_size != 0 && (m.len() & 0xFFFF_FFFF) as u32 != t.file_size {
                    if existing(resolver, &rel).is_some_and(|p| has_serato_tags(&p)) {
                        report.serato_tagged.push(rel.clone());
                    }
                    report.size_mismatch.push(rel.clone());
                }
            }
        }
        report.tracks.push(LibraryTrack {
            id: t.id,
            title: t.title,
            artist: t.artist,
            path: rel,
            db_sample_rate: Some(t.sample_rate).filter(|&r| r > 0),
            db_bit_depth: Some(t.sample_depth).filter(|&d| d > 0),
            db_file_size: Some(t.file_size as u64).filter(|&s| s > 0),
            db_duration_secs: Some(t.duration_secs).filter(|&d| d > 0),
            db_bitrate_kbps: Some(t.bitrate).filter(|&b| b > 0),
        });
    }
    report.playlists = pdb
        .playlists
        .into_iter()
        .map(|p| PlaylistSummary {
            id: p.id,
            parent_id: p.parent_id,
            name: p.name,
            is_folder: p.is_folder,
            track_ids: p.track_ids,
        })
        .collect();
    report
}

/// Audio files on the volume, skipping OS litter and vendor analysis folders.
pub fn walk_audio_files(root: &Path) -> impl Iterator<Item = (String, PathBuf, u64)> + '_ {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && is_audio_path(e.path()))
        .filter(|e| !e.file_name().to_string_lossy().starts_with("._"))
        .map(move |e| {
            let rel = relative(root, e.path());
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            (rel, e.into_path(), size)
        })
}

pub(crate) fn is_ignored_dir(e: &walkdir::DirEntry) -> bool {
    if e.depth() == 0 || !e.file_type().is_dir() {
        return false;
    }
    let name = e.file_name().to_string_lossy();
    matches!(
        name.as_ref(),
        ".Spotlight-V100"
            | ".Trashes"
            | ".fseventsd"
            | ".TemporaryItems"
            | "System Volume Information"
            | "$RECYCLE.BIN"
            | ".boothready"
    ) || (e.depth() == 2 && name.eq_ignore_ascii_case("USBANLZ"))
}

/// macOS AppleDouble (`._name`) files that sit next to audio files.
/// Players browsing folders show them as unplayable tracks.
pub fn apple_double_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e))
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file() && e.file_name().to_string_lossy().starts_with("._") && is_audio_path(e.path())
        })
        .map(|e| e.into_path())
        .collect()
}

pub fn count_apple_double(root: &Path) -> u32 {
    apple_double_files(root).len() as u32
}

pub(crate) fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests;
