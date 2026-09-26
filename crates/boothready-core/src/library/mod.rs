//! Detection and validation of DJ libraries on a mounted volume.
//!
//! Integration levels follow the PRD: rekordbox Device Library is read and
//! validated (level 1-2), OneLibrary is detected with heuristics only (level
//! 0) because the database is encrypted, Engine DJ databases are read and
//! validated, Serato crates are detected.

mod engine;
pub mod pdb;
#[cfg(any(test, feature = "fixtures"))]
pub mod pdb_fixture;
mod resolve;

use crate::audio::is_audio_path;
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RekordboxReport {
    pub device_library: Option<DeviceLibraryReport>,
    pub one_library: Option<OneLibraryReport>,
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
                if !dl.size_mismatch.is_empty() {
                    p.push(format!("{} tracks changed on disk after the rekordbox export", dl.size_mismatch.len()));
                }
            }
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

fn scan_rekordbox(resolver: &mut PathResolver) -> Option<RekordboxReport> {
    let rb_dir = existing(resolver, REKORDBOX_DIR)?;
    if !rb_dir.is_dir() {
        return None;
    }
    let mut warnings = Vec::new();
    let device_library = existing(resolver, DEVICE_LIBRARY_FILE).map(|p| read_device_library(&p, resolver));
    let one_library = existing(resolver, ONE_LIBRARY_FILE).and_then(|p| {
        let meta = fs::metadata(&p).ok()?;
        let mut head = [0u8; 16];
        let n = fs::File::open(&p).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
        Some(OneLibraryReport {
            modified_unix: modified_unix(&meta),
            size_bytes: meta.len(),
            encrypted: n == 16 && &head != b"SQLite format 3\0",
        })
    });
    if let Some(ol) = &one_library {
        if ol.size_bytes == 0 {
            warnings.push("rekordbox OneLibrary database is empty (export interrupted?)".into());
        }
    }
    if let (Some(dl), Some(ol)) = (&device_library, &one_library) {
        if let (Some(a), Some(b)) = (dl.modified_unix, ol.modified_unix) {
            // rekordbox writes both databases during one export. A large gap
            // suggests one of them was left over from an earlier export.
            if a.abs_diff(b) > 3600 {
                warnings.push(
                    "The Device Library and OneLibrary databases were written at different times; one of them may be out of date".into(),
                );
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
    Some(RekordboxReport { device_library, one_library, analysis_files, has_settings, warnings })
}

fn read_device_library(path: &Path, resolver: &mut PathResolver) -> DeviceLibraryReport {
    let meta = fs::metadata(path).ok();
    let mut report = DeviceLibraryReport {
        modified_unix: meta.as_ref().and_then(modified_unix),
        size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        has_ext: existing(resolver, DEVICE_LIBRARY_EXT_FILE).is_some(),
        tracks: vec![],
        playlists: vec![],
        missing: vec![],
        size_mismatch: vec![],
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
