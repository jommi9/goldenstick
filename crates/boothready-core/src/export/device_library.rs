//! Write a complete rekordbox Device Library tree from local audio files.
//!
//! The flow follows djbooth's `exportLegacyDeviceLibrary`
//! (`packages/rekordbox-ts/src/devicelib/export.ts`) with the master
//! database replaced by the files themselves: track metadata comes from the
//! audio tags and header, the beat grid, key and waveforms from
//! `djbooth-analyzer`, and the database from [`DevicePdbWriter`]. Everything
//! is built in a staging folder inside the target and moved into place once
//! the database has been read back through BoothReady's own reader.
//!
//! Compared with djbooth this writer always produces a fresh export: there
//! is no ownership marker, no differential reuse of files already on the
//! media and no history import. A target that already holds a rekordbox
//! export is refused, in line with the rule in `docs/EXPORT-PROVIDERS.md`
//! that a smaller selection regenerates the whole export.

use crate::audio::tags::{read_tags, TrackTags};
use crate::audio::{probe_file, AudioInfo};
use crate::library::pdb::parse_pdb;
use crate::library::pdb_writer::{
    file_type_for_path, inspect_pdb, usb_analysis_path, DevicePdbWriter, TrackInput, WriterError,
};
use crate::library::{ANALYSIS_DIR, DEVICE_LIBRARY_FILE};
use djbooth_analyzer::{
    analyze_audio_file_with_options, build_path_tag, AnalysisOptions, AnlzFile, AnlzTag, RhythmMode,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The three analysis files rekordbox writes per track.
const ANALYSIS_EXTENSIONS: [&str; 3] = ["DAT", "EXT", "2EX"];

/// One playlist to write: a name and the positions of its tracks in
/// [`ExportRequest::tracks`], in playing order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSpec {
    pub name: String,
    pub tracks: Vec<usize>,
}

/// How tracks are analysed before they go into the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Analysis {
    /// Beat grid, key, phrases and waveforms from `djbooth-analyzer`.
    Normal,
    /// The same, with a per-beat variable tempo grid.
    Dynamic,
    /// Dynamic only when the tempo spread calls for it.
    Auto,
    /// No analysis files; tempo and key come from the tags when present.
    Skip,
}

impl Analysis {
    fn rhythm_mode(self) -> Option<RhythmMode> {
        match self {
            Analysis::Normal => Some(RhythmMode::Normal),
            Analysis::Dynamic => Some(RhythmMode::Dynamic),
            Analysis::Auto => Some(RhythmMode::Auto),
            Analysis::Skip => None,
        }
    }
}

/// What to export and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// Local folder that receives `Contents/` and `PIONEER/`. It must not
    /// already hold a rekordbox export.
    pub root: PathBuf,
    /// Audio files, each exported once. Their order fixes the track IDs.
    pub tracks: Vec<PathBuf>,
    pub playlists: Vec<PlaylistSpec>,
    pub analysis: Analysis,
}

/// Progress of one export, for a terminal or a progress bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportProgress {
    pub phase: &'static str,
    pub done: usize,
    pub total: usize,
    /// File name being worked on, or empty.
    pub current: String,
}

/// One track as it ended up in the tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportedTrack {
    pub id: u32,
    /// Volume-absolute path on the exported media.
    pub usb_path: String,
    pub title: String,
    pub artist: Option<String>,
    pub tempo_bpm: Option<f64>,
    pub key: Option<String>,
    pub duration_secs: u16,
    pub analysis_files: u8,
}

/// The finished export. Source paths stay out of it so the report can be
/// shared; see the privacy rules in `docs/EXPORT-PROVIDERS.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportReport {
    pub root: PathBuf,
    pub pdb_path: PathBuf,
    pub tracks: Vec<ExportedTrack>,
    pub playlists: usize,
    pub analysis_files: usize,
    pub bytes_copied: u64,
    pub elapsed_ms: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("{0}")]
    Request(String),
    #[error("export target {0}")]
    Target(String),
    #[error("{0}")]
    Track(String),
    #[error("writing export.pdb: {0}")]
    Writer(#[from] WriterError),
    #[error("the generated export.pdb did not read back correctly: {0}")]
    Verification(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

type Result<T> = std::result::Result<T, ExportError>;

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> ExportError {
    move |source| ExportError::Io { context: context.into(), source }
}

/// Removes the staging folder when the export ends, however it ends.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Build the tree described by `request`. `progress` is called as phases
/// advance; the whole export fails on the first unreadable track.
pub fn export_device_library(
    request: &ExportRequest,
    mut progress: impl FnMut(&ExportProgress),
) -> Result<ExportReport> {
    let started = Instant::now();
    validate_request(request)?;
    let root = validate_target(&request.root)?;
    let staging_dir = root.join(format!(".boothready-export-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging_dir).map_err(io_err(format!("creating {}", staging_dir.display())))?;
    let staging = Staging(staging_dir.clone());

    let mut writer = DevicePdbWriter::new()?;
    let mut tracks = Vec::with_capacity(request.tracks.len());
    let mut warnings = Vec::new();
    let mut bytes_copied = 0u64;
    let mut analysis_files = 0usize;
    let total = request.tracks.len();
    let date_added = today();
    for (index, source) in request.tracks.iter().enumerate() {
        let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        progress(&ExportProgress { phase: "analyze", done: index, total, current: name.clone() });
        let id = index as u32 + 1;
        let prepared = prepare_track(source, id)?;
        let analysis = match request.analysis.rhythm_mode() {
            Some(mode) => match analyze(source, &prepared.usb_path, mode) {
                Ok(files) => Some(files),
                Err(e) => {
                    warnings.push(format!("{name}: analysis failed, so it has no beat grid or waveform ({e})"));
                    None
                }
            },
            None => None,
        };
        let mut files_written = 0u8;
        if let Some(generated) = &analysis {
            let dir = media_path(&staging_dir, prepared.analysis_dir());
            fs::create_dir_all(&dir).map_err(io_err(format!("creating {}", dir.display())))?;
            for (ext, bytes) in ANALYSIS_EXTENSIONS.iter().zip(&generated.files) {
                let path = dir.join(format!("ANLZ0000.{ext}"));
                fs::write(&path, bytes).map_err(io_err(format!("writing {}", path.display())))?;
                bytes_copied += bytes.len() as u64;
                files_written += 1;
            }
            analysis_files += usize::from(files_written);
        }

        progress(&ExportProgress { phase: "copy", done: index, total, current: name.clone() });
        let audio_target = media_path(&staging_dir, &prepared.usb_path);
        if let Some(parent) = audio_target.parent() {
            fs::create_dir_all(parent).map_err(io_err(format!("creating {}", parent.display())))?;
        }
        bytes_copied += fs::copy(source, &audio_target).map_err(io_err(format!(
            "copying {} to {}",
            source.display(),
            audio_target.display()
        )))?;

        let tempo_bpm = analysis.as_ref().map(|a| a.tempo_bpm).or(prepared.tags.bpm.map(f64::from));
        let key = analysis.as_ref().map(|a| a.key.clone()).or_else(|| prepared.tags.key.clone());
        let duration = analysis
            .as_ref()
            .map(|a| a.duration_secs)
            .or(prepared.info.duration_secs)
            .map(|d| d.round().clamp(0.0, f64::from(u16::MAX)) as u16)
            .unwrap_or(0);
        let title = prepared.tags.title.clone().unwrap_or_else(|| prepared.stem.clone());
        let input = TrackInput {
            id: Some(id),
            title: title.clone(),
            file_path: prepared.usb_path.clone(),
            filename: Some(prepared.filename.clone()),
            artist: prepared.tags.artist.clone(),
            album: prepared.tags.album.clone(),
            genre: prepared.tags.genre.clone(),
            key: key.clone(),
            analyze_path: if analysis.is_some() { prepared.analysis_path.clone() } else { String::new() },
            date_added: date_added.clone(),
            tempo: tempo_bpm.map(|b| (b * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32).unwrap_or(0),
            duration,
            year: prepared.tags.year.unwrap_or(0),
            bitrate: prepared.info.bitrate_kbps.unwrap_or(0),
            sample_rate: prepared.info.sample_rate.unwrap_or(44_100),
            sample_depth: prepared.info.bit_depth.unwrap_or(16),
            file_size: prepared.file_size,
            file_type: Some(prepared.file_type),
            ..TrackInput::default()
        };
        writer.add_track(&input)?;
        tracks.push(ExportedTrack {
            id,
            usb_path: prepared.usb_path,
            title,
            artist: prepared.tags.artist,
            tempo_bpm,
            key,
            duration_secs: duration,
            analysis_files: files_written,
        });
    }

    progress(&ExportProgress { phase: "database", done: total, total, current: String::new() });
    for playlist in &request.playlists {
        let node = writer.create_playlist(&playlist.name, 0, false)?;
        for (position, &track) in playlist.tracks.iter().enumerate() {
            writer.add_to_playlist(node, track as u32 + 1, Some(position as u32 + 1))?;
        }
    }
    let pdb = writer.build()?;
    verify_pdb(&pdb, request, &tracks)?;
    let staged_pdb = staging_dir.join(DEVICE_LIBRARY_FILE);
    if let Some(parent) = staged_pdb.parent() {
        fs::create_dir_all(parent).map_err(io_err(format!("creating {}", parent.display())))?;
    }
    fs::write(&staged_pdb, &pdb).map_err(io_err(format!("writing {}", staged_pdb.display())))?;

    progress(&ExportProgress { phase: "install", done: total, total, current: String::new() });
    install(&staging_dir, &root)?;
    drop(staging);
    Ok(ExportReport {
        pdb_path: root.join(DEVICE_LIBRARY_FILE),
        root,
        tracks,
        playlists: request.playlists.len(),
        analysis_files,
        bytes_copied,
        elapsed_ms: started.elapsed().as_millis() as u64,
        warnings,
    })
}

fn validate_request(request: &ExportRequest) -> Result<()> {
    if request.tracks.is_empty() {
        return Err(ExportError::Request("nothing to export: no tracks were given".into()));
    }
    let mut seen = BTreeSet::new();
    for track in &request.tracks {
        if !seen.insert(track) {
            return Err(ExportError::Request(format!("{} is listed twice", track.display())));
        }
    }
    for playlist in &request.playlists {
        if playlist.name.trim().is_empty() {
            return Err(ExportError::Request("a playlist has no name".into()));
        }
        if let Some(bad) = playlist.tracks.iter().find(|&&i| i >= request.tracks.len()) {
            return Err(ExportError::Request(format!(
                "playlist '{}' refers to track {bad}, but only {} tracks were given",
                playlist.name,
                request.tracks.len()
            )));
        }
    }
    Ok(())
}

/// The target must be a real, local folder without a rekordbox export in it.
fn validate_target(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root).map_err(io_err(format!("creating {}", root.display())))?;
    let meta = fs::symlink_metadata(root).map_err(io_err(format!("reading {}", root.display())))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(ExportError::Target(format!("{} must be a real folder", root.display())));
    }
    let root = root.canonicalize().map_err(io_err(format!("resolving {}", root.display())))?;
    if root.parent().is_none() {
        return Err(ExportError::Target(format!("{} is a filesystem root", root.display())));
    }
    for existing in ["Contents", ANALYSIS_DIR, DEVICE_LIBRARY_FILE] {
        if root.join(existing).exists() {
            return Err(ExportError::Target(format!(
                "{} already holds a rekordbox export ({existing}); export into an empty folder",
                root.display()
            )));
        }
    }
    Ok(root)
}

struct PreparedTrack {
    stem: String,
    filename: String,
    usb_path: String,
    analysis_path: String,
    file_type: u16,
    file_size: u32,
    info: AudioInfo,
    tags: TrackTags,
}

impl PreparedTrack {
    fn analysis_dir(&self) -> &str {
        self.analysis_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(&self.analysis_path)
    }
}

fn prepare_track(source: &Path, id: u32) -> Result<PreparedTrack> {
    let display = source.display();
    let meta = fs::symlink_metadata(source).map_err(io_err(format!("reading {display}")))?;
    if !meta.is_file() {
        return Err(ExportError::Track(format!("{display} is not a regular file")));
    }
    let file_size = u32::try_from(meta.len())
        .map_err(|_| ExportError::Track(format!("{display} is larger than the 4 GiB a Device Library can record")))?;
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ExportError::Track(format!("{display} has no usable file name")))?;
    let file_type = file_type_for_path(name)
        .ok_or_else(|| ExportError::Track(format!("{display}: a Device Library cannot hold this audio format")))?;
    let info = probe_file(source).map_err(|e| ExportError::Track(format!("{display}: {e}")))?;
    let tags = read_tags(source).unwrap_or_default();
    let filename = safe_filename(name, id);
    let stem = filename.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| filename.clone());
    let usb_path = format!("/Contents/{id:08X}/{filename}");
    let analysis_path = usb_analysis_path(&usb_path)?;
    Ok(PreparedTrack { stem, filename, usb_path, analysis_path, file_type, file_size, info, tags })
}

struct GeneratedFiles {
    /// DAT, EXT and 2EX bytes, in that order.
    files: [Vec<u8>; 3],
    tempo_bpm: f64,
    key: String,
    duration_secs: f64,
}

/// Analyse one file and point its analysis at where the audio will be on the media.
fn analyze(source: &Path, usb_path: &str, mode: RhythmMode) -> std::result::Result<GeneratedFiles, String> {
    let options = AnalysisOptions::automatic().with_rhythm_mode(mode);
    let generated = analyze_audio_file_with_options(source, options).map_err(|e| format!("{e:#}"))?;
    let files = [&generated.dat, &generated.ext, &generated.two_ex]
        .map(|file| with_path(file, usb_path).and_then(|f| f.to_bytes().map_err(|e| format!("{e:#}"))));
    let [dat, ext, two_ex] = files;
    Ok(GeneratedFiles {
        files: [dat?, ext?, two_ex?],
        tempo_bpm: generated.beat.bpm,
        key: generated.key.key,
        duration_secs: generated.duration_seconds,
    })
}

/// Replace the PPTH section of an analysis file, keeping every other tag.
fn with_path(file: &AnlzFile, usb_path: &str) -> std::result::Result<AnlzFile, String> {
    let tags = file
        .tags()
        .iter()
        .map(|tag| if tag.kind() == *b"PPTH" { build_path_tag(usb_path) } else { Ok::<AnlzTag, _>(tag.clone()) })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| format!("{e:#}"))?;
    AnlzFile::from_tags(tags).map_err(|e| format!("{e:#}"))
}

/// Read the database back through BoothReady's reader before anything is installed.
fn verify_pdb(pdb: &[u8], request: &ExportRequest, tracks: &[ExportedTrack]) -> Result<()> {
    let parsed = parse_pdb(pdb).map_err(|e| ExportError::Verification(e.to_string()))?;
    if !parsed.warnings.is_empty() {
        return Err(ExportError::Verification(parsed.warnings.join("; ")));
    }
    if parsed.tracks.len() != tracks.len() {
        return Err(ExportError::Verification(format!("{} tracks instead of {}", parsed.tracks.len(), tracks.len())));
    }
    for (written, read) in tracks.iter().zip(&parsed.tracks) {
        if read.id != written.id || read.file_path != written.usb_path || read.title != written.title {
            return Err(ExportError::Verification(format!(
                "track {} came back as {} at {}",
                written.id, read.id, read.file_path
            )));
        }
        if read.artist != written.artist {
            return Err(ExportError::Verification(format!("track {} lost its artist", written.id)));
        }
    }
    if parsed.playlists.len() != request.playlists.len() {
        return Err(ExportError::Verification(format!(
            "{} playlists instead of {}",
            parsed.playlists.len(),
            request.playlists.len()
        )));
    }
    for (spec, read) in request.playlists.iter().zip(&parsed.playlists) {
        let expected: Vec<u32> = spec.tracks.iter().map(|&i| i as u32 + 1).collect();
        if read.name != spec.name || read.track_ids != expected {
            return Err(ExportError::Verification(format!(
                "playlist '{}' came back as '{}' with other tracks",
                spec.name, read.name
            )));
        }
    }
    let inspection = inspect_pdb(pdb).map_err(|e| ExportError::Verification(e.to_string()))?;
    if inspection.tracks.len() != tracks.len() || !inspection.histories.is_empty() {
        return Err(ExportError::Verification("djbooth's inspection disagrees with the reader".into()));
    }
    Ok(())
}

/// Move the staged `Contents` and `PIONEER` folders into the target.
fn install(staging: &Path, root: &Path) -> Result<()> {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for name in ["Contents", "PIONEER"] {
        let from = staging.join(name);
        if !from.exists() {
            continue;
        }
        let to = root.join(name);
        if let Err(e) = fs::rename(&from, &to) {
            for (to, from) in moved.iter().rev() {
                let _ = fs::rename(to, from);
            }
            return Err(ExportError::Io { context: format!("moving {} into {}", name, root.display()), source: e });
        }
        moved.push((to, from));
    }
    Ok(())
}

/// A path inside `root` for a volume-absolute media path.
fn media_path(root: &Path, usb_path: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for part in usb_path.split('/').filter(|p| !p.is_empty()) {
        out.push(part);
    }
    out
}

/// A file name every player filesystem accepts, the way djbooth cleans one.
pub fn safe_filename(value: &str, track_id: u32) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let trimmed: String = cleaned.trim_end_matches(['.', ' ']).chars().take(180).collect();
    if trimmed.is_empty() {
        format!("track-{track_id:08}")
    } else {
        trimmed
    }
}

/// Today's date as `YYYY-MM-DD` in UTC, the form rekordbox stores.
fn today() -> String {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
