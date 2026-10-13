//! The music library: folders the user added as sources, the last scan of
//! each and how their tracks stand against the selected players.
//!
//! Nothing here writes to a source folder. Records and scans are kept as
//! JSON in the app's own data folder, so a source on an unplugged drive can
//! still be browsed from its last scan.

use boothready_core::audio::{AudioInfo, Codec, Container};
use boothready_core::rules::DeviceProfile;
use boothready_core::scan::TrackProbe;
use boothready_core::sources::{
    cloud_provider, count_states, device_for_path, judge, PlayerIssue, SourceScan, SourceTotals, SourceTrack,
    StateCounts, TrackState,
};
use boothready_model::PhysicalDevice;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a source folder is stored, as the user would describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Location {
    Computer,
    External { name: String },
    Cloud { provider: String },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub id: String,
    pub path: PathBuf,
    pub added_unix: u64,
    /// As seen when the folder was last reachable.
    pub location: Location,
    pub scanned_unix: Option<u64>,
    pub totals: Option<SourceTotals>,
    #[serde(default)]
    pub skipped: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibrarySource {
    pub id: String,
    pub name: String,
    pub path: String,
    pub location: Location,
    /// The folder can be read right now.
    pub connected: bool,
    pub scanning: bool,
    pub scanned_unix: Option<u64>,
    pub totals: Option<SourceTotals>,
    /// Against the selected players, from the last scan.
    pub states: Option<StateCounts>,
    /// Folders or files the last scan could not read.
    pub skipped: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryTrack {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub format: String,
    pub detail: String,
    pub state: TrackState,
    /// Why the file is unreadable, when it is.
    pub note: Option<String>,
    pub issues: Vec<PlayerIssue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryPage {
    pub total: u32,
    pub tracks: Vec<LibraryTrack>,
}

#[derive(Default)]
pub struct LibraryState {
    /// `None` keeps everything in memory, as demo mode does.
    dir: Option<PathBuf>,
    sources: Vec<SourceRecord>,
    scans: HashMap<String, Arc<SourceScan>>,
    scanning: HashSet<String>,
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Describe where `path` is stored. `devices` is the platform's device list.
pub fn locate(path: &Path, devices: &[PhysicalDevice]) -> Location {
    if let Some(provider) = cloud_provider(path) {
        return Location::Cloud { provider: provider.to_string() };
    }
    match device_for_path(path, devices) {
        Some(device) if device.is_system => Location::Computer,
        Some(device) => {
            let volume = device
                .volumes
                .iter()
                .filter(|v| v.mount_point.as_ref().is_some_and(|m| path.starts_with(m)))
                .max_by_key(|v| v.mount_point.as_ref().map(|m| m.components().count()));
            let name = volume
                .and_then(|v| v.label.clone())
                .or_else(|| device.storage_model.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "External drive".to_string());
            Location::External { name: name.trim().to_string() }
        }
        None => Location::Unknown,
    }
}

/// The folder as the operating system names it, with symlinks resolved so a
/// shortcut to a cloud folder is recognised for what it points at.
pub fn resolve_folder(path: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    let resolved = std::path::absolute(path);
    #[cfg(not(windows))]
    let resolved = path.canonicalize();
    let resolved = resolved.map_err(|_| "That folder can't be opened.".to_string())?;
    if !resolved.is_dir() {
        return Err("Choose a folder, not a file.".to_string());
    }
    Ok(resolved)
}

impl LibraryState {
    /// Load the saved sources from `dir`. A missing or damaged list starts empty.
    pub fn load(dir: PathBuf) -> LibraryState {
        let sources = std::fs::read(dir.join("sources.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        LibraryState { dir: Some(dir), sources, ..LibraryState::default() }
    }

    /// The simulated library for demo mode: a folder on the computer, a
    /// Google Drive folder with tracks still online, and an archive drive
    /// that isn't plugged in. Kept in memory only.
    pub fn demo(root: &Path) -> Result<LibraryState, String> {
        let folders = boothready_core::demo::create_music_sources(root).map_err(|e| e.to_string())?;
        let mut state = LibraryState::default();
        let never_cancelled = std::sync::atomic::AtomicBool::new(false);
        for (id, folder) in ["demo-computer", "demo-cloud"].iter().zip(&folders) {
            let placeholder = boothready_core::sources::listed_placeholder(folder);
            let mut scan = boothready_core::sources::scan_source(folder, &placeholder, &never_cancelled, |_| {})
                .map_err(|e| e.to_string())?;
            demo_sizes(&mut scan);
            state.sources.push(SourceRecord {
                id: (*id).to_string(),
                path: folder.clone(),
                added_unix: DEMO_TIME,
                location: locate(folder, &[]).or_computer(),
                scanned_unix: Some(DEMO_TIME),
                totals: Some(scan.totals()),
                skipped: 0,
            });
            state.scans.insert((*id).to_string(), Arc::new(scan));
        }
        let archive = state.scans.get("demo-computer").map(|seed| demo_archive(seed)).unwrap_or_default();
        state.sources.push(SourceRecord {
            id: "demo-archive".to_string(),
            path: root.join("Volumes").join("Archive 2TB").join("Music Archive"),
            added_unix: DEMO_TIME,
            location: Location::External { name: "Archive 2TB".to_string() },
            scanned_unix: Some(DEMO_TIME),
            totals: Some(archive.totals()),
            skipped: 0,
        });
        state.scans.insert("demo-archive".to_string(), Arc::new(archive));
        Ok(state)
    }

    fn save_sources(&self) {
        let Some(dir) = &self.dir else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.sources) {
            let _ = std::fs::write(dir.join("sources.json"), bytes);
        }
    }

    fn scan_file(&self, id: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(format!("scan-{id}.json")))
    }

    /// The last scan of a source, read from disk the first time it is needed.
    pub fn scan(&mut self, id: &str) -> Option<Arc<SourceScan>> {
        if let Some(scan) = self.scans.get(id) {
            return Some(scan.clone());
        }
        let bytes = std::fs::read(self.scan_file(id)?).ok()?;
        let scan: Arc<SourceScan> = Arc::new(serde_json::from_slice(&bytes).ok()?);
        self.scans.insert(id.to_string(), scan.clone());
        Some(scan)
    }

    pub fn record(&self, id: &str) -> Result<SourceRecord, String> {
        self.sources.iter().find(|s| s.id == id).cloned().ok_or_else(|| "That music source was removed.".to_string())
    }

    /// Add a folder that [`resolve_folder`] accepted.
    pub fn add(&mut self, path: PathBuf, location: Location, now: u64) -> Result<String, String> {
        for existing in &self.sources {
            if existing.path == path {
                return Err("That folder is already in your library.".to_string());
            }
            if path.starts_with(&existing.path) {
                return Err(format!(
                    "That folder is inside \"{}\", which is already in your library.",
                    name_of(&existing.path)
                ));
            }
            if existing.path.starts_with(&path) {
                return Err(format!(
                    "That folder contains \"{}\", which is already in your library. Remove it first to add the whole folder.",
                    name_of(&existing.path)
                ));
            }
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.sources.push(SourceRecord {
            id: id.clone(),
            path,
            added_unix: now,
            location,
            scanned_unix: None,
            totals: None,
            skipped: 0,
        });
        self.save_sources();
        Ok(id)
    }

    /// Forget a source and its saved scan. The folder itself is not touched.
    pub fn remove(&mut self, id: &str) {
        self.sources.retain(|s| s.id != id);
        self.scans.remove(id);
        if let Some(file) = self.scan_file(id) {
            let _ = std::fs::remove_file(file);
        }
        self.save_sources();
    }

    pub fn is_scanning(&self) -> bool {
        !self.scanning.is_empty()
    }

    pub fn set_scanning(&mut self, id: &str, on: bool) {
        if on {
            self.scanning.insert(id.to_string());
        } else {
            self.scanning.remove(id);
        }
    }

    /// Keep a finished scan and what it found about the folder's location.
    pub fn store_scan(&mut self, id: &str, scan: SourceScan, location: Location, now: u64) {
        let Some(record) = self.sources.iter_mut().find(|s| s.id == id) else { return };
        record.location = location;
        record.scanned_unix = Some(now);
        record.totals = Some(scan.totals());
        record.skipped = scan.skipped;
        if let Some(file) = self.scan_file(id) {
            if let Ok(bytes) = serde_json::to_vec(&scan) {
                let _ = std::fs::write(file, bytes);
            }
        }
        self.scans.insert(id.to_string(), Arc::new(scan));
        self.save_sources();
    }

    pub fn list(&mut self, targets: &[&DeviceProfile]) -> Vec<LibrarySource> {
        let records = self.sources.clone();
        records
            .into_iter()
            .map(|record| {
                let states = self.scan(&record.id).map(|scan| count_states(&scan, targets));
                LibrarySource {
                    name: name_of(&record.path),
                    path: record.path.display().to_string(),
                    connected: record.path.is_dir(),
                    scanning: self.scanning.contains(&record.id),
                    location: record.location,
                    scanned_unix: record.scanned_unix,
                    totals: record.totals,
                    states,
                    skipped: record.skipped,
                    id: record.id,
                }
            })
            .collect()
    }

    /// One page of a source's tracks, filtered by state and by a search on
    /// the path. `filter` is a [`TrackState`] name or "all".
    pub fn tracks(
        &mut self,
        id: &str,
        targets: &[&DeviceProfile],
        filter: &str,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<LibraryPage, String> {
        self.record(id)?;
        let Some(scan) = self.scan(id) else {
            return Ok(LibraryPage { total: 0, tracks: Vec::new() });
        };
        let query = query.trim().to_lowercase();
        let mut total = 0u32;
        let mut tracks = Vec::new();
        for track in &scan.tracks {
            if !query.is_empty() && !track.path.to_lowercase().contains(&query) {
                continue;
            }
            let (state, issues) = judge(track, targets);
            if filter != "all" && state_name(state) != filter {
                continue;
            }
            if total as usize >= offset && tracks.len() < limit {
                tracks.push(track_row(track, state, issues));
            }
            total += 1;
        }
        Ok(LibraryPage { total, tracks })
    }
}

/// A fixed timestamp for the simulated library, so demo output is stable.
const DEMO_TIME: u64 = 1_790_000_000;

impl Location {
    /// Demo folders live in a temporary directory on no known device.
    pub(crate) fn or_computer(self) -> Location {
        if self == Location::Unknown {
            Location::Computer
        } else {
            self
        }
    }
}

/// Demo files are a few kilobytes of valid headers. Give them the sizes real
/// tracks have, so the simulated library reads like a real one. A damaged
/// file keeps its true size.
pub(crate) fn demo_sizes(scan: &mut SourceScan) {
    for track in &mut scan.tracks {
        let lossless = match &track.probe {
            Some(TrackProbe::Ok(info)) => is_lossless(info),
            Some(TrackProbe::Unreadable(_)) => continue,
            None => [".wav", ".aiff", ".aif", ".flac"].iter().any(|e| track.path.to_lowercase().ends_with(e)),
        };
        // FNV-1a, so a track keeps the same size on every run.
        let hash = track
            .path
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        track.size = if lossless { 38_000_000 + hash % 40_000_000 } else { 7_000_000 + hash % 8_000_000 };
    }
}

/// A saved scan for the simulated archive drive that isn't plugged in,
/// built from the formats in the simulated computer folder.
fn demo_archive(seed: &SourceScan) -> SourceScan {
    let playable: Vec<&SourceTrack> =
        seed.tracks.iter().filter(|t| matches!(t.probe, Some(TrackProbe::Ok(_)))).collect();
    let crates = ["House", "Techno", "Disco", "Breaks", "Ambient", "Edits"];
    let mut scan = SourceScan::default();
    if playable.is_empty() {
        return scan;
    }
    for n in 0..144usize {
        let like = playable[(n * 7) % playable.len()];
        let extension = like.path.rsplit_once('.').map(|(_, e)| e).unwrap_or("mp3");
        scan.tracks.push(SourceTrack {
            path: format!("{}/{}/Archive Track {:03}.{extension}", crates[n % crates.len()], 2012 + (n / 12), n + 1),
            size: 0,
            modified_unix: Some(DEMO_TIME - 86_400 * (n as u64 + 30)),
            availability: like.availability,
            probe: like.probe.clone(),
        });
    }
    scan.tracks.sort_by(|a, b| a.path.cmp(&b.path));
    demo_sizes(&mut scan);
    scan
}

fn is_lossless(info: &AudioInfo) -> bool {
    matches!(info.container, Container::Wav | Container::Aiff | Container::Flac) || info.codec == Codec::Alac
}

pub fn state_name(state: TrackState) -> &'static str {
    match state {
        TrackState::Ready => "ready",
        TrackState::CloudOnly => "cloud_only",
        TrackState::Blocked => "blocked",
        TrackState::Unchecked => "unchecked",
        TrackState::Unreadable => "unreadable",
    }
}

fn format_label(info: &AudioInfo) -> String {
    match info.container {
        Container::Wav => "WAV".to_string(),
        Container::Aiff => "AIFF".to_string(),
        Container::Flac => "FLAC".to_string(),
        Container::Mp3 => "MP3".to_string(),
        Container::Mp4 | Container::Ogg => info.codec.label().to_string(),
    }
}

fn detail_label(info: &AudioInfo) -> String {
    let mut parts = Vec::new();
    if let Some(rate) = info.sample_rate {
        let khz = f64::from(rate) / 1000.0;
        parts.push(if rate % 1000 == 0 { format!("{khz:.0} kHz") } else { format!("{khz:.1} kHz") });
    }
    match (info.bit_depth, info.bitrate_kbps) {
        (Some(bits), _) if is_lossless(info) => parts.push(format!("{bits}-bit")),
        (_, Some(kbps)) => parts.push(format!("{kbps} kbps")),
        (Some(bits), None) => parts.push(format!("{bits}-bit")),
        (None, None) => {}
    }
    parts.join(", ")
}

fn track_row(track: &SourceTrack, state: TrackState, issues: Vec<PlayerIssue>) -> LibraryTrack {
    let (folder, name) = match track.path.rsplit_once('/') {
        Some((folder, name)) => (folder.to_string(), name.to_string()),
        None => (String::new(), track.path.clone()),
    };
    let extension = name.rsplit_once('.').map(|(_, e)| e.to_uppercase()).unwrap_or_default();
    let (format, detail, note) = match &track.probe {
        Some(TrackProbe::Ok(info)) => (format_label(info), detail_label(info), None),
        Some(TrackProbe::Unreadable(reason)) => (extension, String::new(), Some(reason.clone())),
        None => (extension, String::new(), None),
    };
    LibraryTrack { path: track.path.clone(), name, folder, size: track.size, format, detail, state, note, issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boothready_core::rules::Ruleset;

    fn club(rules: &Ruleset) -> Vec<&DeviceProfile> {
        rules.devices_by_id(&rules.preset("unknown_club").unwrap().devices)
    }

    #[test]
    fn demo_library_shows_local_cloud_and_unplugged_sources() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let mut library = LibraryState::demo(root.path()).unwrap();
        let sources = library.list(&club(&rules));
        assert_eq!(sources.len(), 3);

        let computer = &sources[0];
        assert_eq!(computer.location, Location::Computer);
        assert!(computer.connected);
        let states = computer.states.unwrap();
        assert_eq!(states.unreadable, 1);
        assert!(states.blocked > 0, "the hi-res files are rejected by the older players");
        assert_eq!(states.cloud_only, 0);

        let cloud = &sources[1];
        assert_eq!(cloud.location, Location::Cloud { provider: "Google Drive".into() });
        assert_eq!(cloud.states.unwrap().cloud_only, 4);
        assert_eq!(cloud.totals.unwrap().cloud_only, 4);

        // The archive drive is unplugged, and its last scan is still browsable.
        let archive = &sources[2];
        assert!(!archive.connected);
        assert_eq!(archive.totals.unwrap().tracks, 144);
        assert!(archive.totals.unwrap().bytes > 1_000_000_000);
        assert_eq!(archive.states.unwrap().unreadable, 0);
        assert!(archive.states.unwrap().blocked > 0);
    }

    #[test]
    fn tracks_are_filtered_searched_and_paged() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let targets = club(&rules);
        let mut library = LibraryState::demo(root.path()).unwrap();

        let all = library.tracks("demo-computer", &targets, "all", "", 0, 500).unwrap();
        assert_eq!(all.total as usize, all.tracks.len());
        assert_eq!(all.total, 13);

        let page = library.tracks("demo-computer", &targets, "all", "", 10, 5).unwrap();
        assert_eq!((page.total, page.tracks.len()), (13, 3));

        let blocked = library.tracks("demo-computer", &targets, "blocked", "", 0, 500).unwrap();
        assert!(blocked.total > 0);
        assert!(blocked.tracks.iter().all(|t| t.state == TrackState::Blocked && !t.issues.is_empty()));

        let found = library.tracks("demo-computer", &targets, "all", "SUBZERO", 0, 500).unwrap();
        assert!(found.total >= 1);
        assert!(found.tracks.iter().all(|t| t.path.to_lowercase().contains("subzero")));

        let online = library.tracks("demo-cloud", &targets, "cloud_only", "", 0, 500).unwrap();
        assert_eq!(online.total, 4);
        assert!(online.tracks.iter().all(|t| t.detail.is_empty() && t.issues.is_empty()));

        let broken = library.tracks("demo-computer", &targets, "unreadable", "", 0, 500).unwrap();
        assert_eq!(broken.total, 1);
        assert!(broken.tracks[0].note.is_some());

        assert!(library.tracks("missing", &targets, "all", "", 0, 10).is_err());
    }

    #[test]
    fn sources_and_scans_survive_a_restart_and_overlaps_are_refused() {
        let data = tempfile::tempdir().unwrap();
        let music = tempfile::tempdir().unwrap();
        let folder = music.path().canonicalize().unwrap().join("Sets");
        std::fs::create_dir_all(folder.join("Warm-up")).unwrap();
        std::fs::write(folder.join("Warm-up/not-audio.mp3"), b"<html>").unwrap();
        let rules = Ruleset::builtin();
        let dir = data.path().join("library");

        let mut library = LibraryState::load(dir.clone());
        let id = library.add(folder.clone(), Location::Computer, 100).unwrap();
        assert!(library.add(folder.clone(), Location::Computer, 100).is_err());
        assert!(library.add(folder.join("Warm-up"), Location::Computer, 100).is_err());
        assert!(library.add(music.path().canonicalize().unwrap(), Location::Computer, 100).is_err());
        assert_eq!(library.list(&[])[0].states, None);

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let scan = boothready_core::sources::scan_source(&folder, &|_, _| false, &cancel, |_| {}).unwrap();
        library.store_scan(&id, scan, Location::External { name: "Crate".into() }, 200);

        let mut reopened = LibraryState::load(dir);
        let listed = reopened.list(&club(&rules));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].location, Location::External { name: "Crate".into() });
        assert_eq!(listed[0].scanned_unix, Some(200));
        assert_eq!(listed[0].states.unwrap().unreadable, 1);

        reopened.remove(&id);
        assert!(LibraryState::load(data.path().join("library")).list(&[]).is_empty());
        assert!(folder.join("Warm-up/not-audio.mp3").exists(), "removing a source never touches its files");
    }

    #[test]
    fn a_file_is_not_accepted_as_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("track.mp3");
        std::fs::write(&file, b"x").unwrap();
        assert!(resolve_folder(&file).is_err());
        assert!(resolve_folder(&dir.path().join("gone")).is_err());
        assert!(resolve_folder(dir.path()).is_ok());
    }
}
