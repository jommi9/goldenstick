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
    cloud_provider, count_states, device_for_path, judge, scan_source, Availability, PlayerIssue, SourceScan,
    SourceTotals, SourceTrack, StateCounts, TrackState,
};
use boothready_core::staging::{stage_selected, Collection, SourceKind, StageProgress};
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
    /// Set when this source is a local collection BoothReady made.
    #[serde(default)]
    pub collection: Option<CollectionRef>,
}

/// A verified local copy of tracks picked from another source. Its tracks
/// are scanned from `files/` inside `root`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRef {
    pub name: String,
    pub root: PathBuf,
    /// The folder name of the source the tracks came from.
    pub origin: String,
    pub revision: String,
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
    /// The source the tracks were collected from, for a local collection.
    pub collected_from: Option<String>,
}

/// The tracks a collect request covers, taken from a source's last scan.
#[derive(Debug, Clone)]
pub struct Selection {
    pub source: SourceRecord,
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
    pub cloud_only: u32,
    pub cloud_only_bytes: u64,
    /// Matching tracks left out because they are damaged or not audio.
    pub unreadable: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryTrack {
    /// The source the track belongs to, so the collection view can play it.
    pub source: String,
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
    /// States of every track in the folder and search, before the state
    /// filter, so the filter buttons can show their counts.
    pub states: StateCounts,
}

/// The id that stands for every source at once: the whole collection.
pub const ALL_SOURCES: &str = "*";

/// A folder inside a source, for the browser's tree. Folders are listed
/// parent first, each followed by its subfolders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderNode {
    /// Relative to the source, forward slashes.
    pub path: String,
    pub name: String,
    pub depth: u32,
    /// Tracks in this folder and every folder below it.
    pub tracks: u32,
}

/// True when `path` lies inside `folder`. An empty folder holds everything.
fn in_folder(path: &str, folder: &str) -> bool {
    folder.is_empty()
        || (path.len() > folder.len() && path.starts_with(folder) && path.as_bytes()[folder.len()] == b'/')
}

#[derive(Default)]
pub struct LibraryState {
    /// `None` keeps everything in memory, as demo mode does.
    dir: Option<PathBuf>,
    sources: Vec<SourceRecord>,
    scans: HashMap<String, Arc<SourceScan>>,
    scanning: HashSet<String>,
    collecting: bool,
}

/// The name a source is listed under: its collection name or folder name.
fn display_name(record: &SourceRecord) -> String {
    record.collection.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| name_of(&record.path))
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
                collection: None,
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
            collection: None,
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
            collection: None,
        });
        self.save_sources();
        Ok(id)
    }

    /// Add a finished collection as a source of its own, with its scan.
    pub fn add_collection(&mut self, made: CollectionRef, scan: SourceScan, location: Location, now: u64) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.sources.push(SourceRecord {
            id: id.clone(),
            path: made.root.join("files"),
            added_unix: now,
            location: location.clone(),
            scanned_unix: None,
            totals: None,
            skipped: 0,
            collection: Some(made),
        });
        self.store_scan(&id, scan, location, now);
        id
    }

    /// The tracks a collect request covers. `filter` and `query` mean what
    /// they mean in [`LibraryState::tracks`]. Unreadable files are left out,
    /// because copying a damaged file helps nobody.
    pub fn selection(
        &mut self,
        id: &str,
        targets: &[&DeviceProfile],
        filter: &str,
        query: &str,
        folder: &str,
    ) -> Result<Selection, String> {
        let source = self.record(id)?;
        let folder = folder.trim_matches('/');
        let scan = self.scan(id).ok_or_else(|| "Scan this folder first.".to_string())?;
        let query = query.trim().to_lowercase();
        let mut picked =
            Selection { source, paths: Vec::new(), bytes: 0, cloud_only: 0, cloud_only_bytes: 0, unreadable: 0 };
        for track in &scan.tracks {
            if !in_folder(&track.path, folder) {
                continue;
            }
            if !query.is_empty() && !track.path.to_lowercase().contains(&query) {
                continue;
            }
            let state = judge(track, targets).0;
            if filter != "all" && state_name(state) != filter {
                continue;
            }
            if state == TrackState::Unreadable {
                picked.unreadable += 1;
                continue;
            }
            if track.availability == Availability::CloudOnly {
                picked.cloud_only += 1;
                picked.cloud_only_bytes += track.size;
            }
            picked.bytes += track.size;
            picked.paths.push(track.path.split('/').collect());
        }
        Ok(picked)
    }

    /// A scan or a collect is running. Only one runs at a time.
    pub fn is_busy(&self) -> bool {
        self.collecting || !self.scanning.is_empty()
    }

    pub fn set_collecting(&mut self, on: bool) {
        self.collecting = on;
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
                let made = record.collection;
                LibrarySource {
                    name: made.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| name_of(&record.path)),
                    collected_from: made.map(|c| c.origin),
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

    /// One page of a source's tracks, filtered by folder, by a search on the
    /// path and by state. `id` may be [`ALL_SOURCES`], which pages through
    /// every source and puts each source's name in front of the folder.
    /// `filter` is a [`TrackState`] name or "all".
    #[allow(clippy::too_many_arguments)]
    pub fn tracks(
        &mut self,
        id: &str,
        targets: &[&DeviceProfile],
        filter: &str,
        query: &str,
        folder: &str,
        offset: usize,
        limit: usize,
    ) -> Result<LibraryPage, String> {
        let ids: Vec<String> = if id == ALL_SOURCES {
            self.sources.iter().map(|s| s.id.clone()).collect()
        } else {
            self.record(id)?;
            vec![id.to_string()]
        };
        let query = query.trim().to_lowercase();
        let folder = folder.trim_matches('/');
        let mut total = 0u32;
        let mut tracks = Vec::new();
        let mut states = StateCounts::default();
        for sid in &ids {
            let Some(scan) = self.scan(sid) else { continue };
            let label = if id == ALL_SOURCES { self.record(sid).ok().map(|r| display_name(&r)) } else { None };
            for track in &scan.tracks {
                if !in_folder(&track.path, folder) {
                    continue;
                }
                if !query.is_empty() && !track.path.to_lowercase().contains(&query) {
                    continue;
                }
                let (state, issues) = judge(track, targets);
                states.add(state);
                if filter != "all" && state_name(state) != filter {
                    continue;
                }
                if total as usize >= offset && tracks.len() < limit {
                    let mut row = track_row(sid, track, state, issues);
                    if let Some(source) = &label {
                        row.folder =
                            if row.folder.is_empty() { source.clone() } else { format!("{source}/{}", row.folder) };
                    }
                    tracks.push(row);
                }
                total += 1;
            }
        }
        Ok(LibraryPage { total, tracks, states })
    }

    /// The file behind a track, for playing it. The path must name an audio
    /// file inside the source folder: named components only, no symlink out.
    pub fn playable_path(&mut self, id: &str, path: &str) -> Result<PathBuf, String> {
        let record = self.record(id)?;
        let relative = Path::new(path);
        if path.is_empty() || !relative.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            return Err("That isn't a track in this source.".to_string());
        }
        if !boothready_core::audio::is_audio_path(relative) {
            return Err("That file isn't audio.".to_string());
        }
        let root = record.path.canonicalize().map_err(|_| "The source folder can't be reached.".to_string())?;
        let file = root.join(relative).canonicalize().map_err(|_| "That track isn't there any more.".to_string())?;
        if !file.starts_with(&root) || !file.is_file() {
            return Err("That isn't a track in this source.".to_string());
        }
        Ok(file)
    }

    /// The folders inside a source with the number of tracks below each.
    pub fn folders(&mut self, id: &str) -> Result<Vec<FolderNode>, String> {
        self.record(id)?;
        let Some(scan) = self.scan(id) else { return Ok(Vec::new()) };
        // Keyed by components, so a folder sorts directly before its children.
        let mut counts: std::collections::BTreeMap<Vec<String>, u32> = std::collections::BTreeMap::new();
        for track in &scan.tracks {
            let parts: Vec<&str> = track.path.split('/').collect();
            for end in 1..parts.len() {
                *counts.entry(parts[..end].iter().map(|p| p.to_string()).collect()).or_default() += 1;
            }
        }
        Ok(counts
            .into_iter()
            .map(|(parts, tracks)| FolderNode {
                name: parts.last().cloned().unwrap_or_default(),
                depth: parts.len() as u32 - 1,
                path: parts.join("/"),
                tracks,
            })
            .collect())
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

/// A folder name for a collection, made from what the user typed. Anything
/// outside letters, digits, spaces and a few marks becomes a dash, so the
/// name is valid on FAT, NTFS and APFS alike.
pub fn collection_folder_name(typed: &str) -> Result<String, String> {
    let cleaned: String = typed
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_().,&'".contains(c) { c } else { '-' })
        .take(80)
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '-' || c == '.') {
        return Err("Give the collection a name.".to_string());
    }
    Ok(cleaned)
}

/// The calendar date of a Unix time, as YYYY-MM-DD in UTC.
pub fn date_of(unix: u64) -> String {
    // Civil-from-days, after Howard Hinnant's algorithm.
    let days = (unix / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Bytes a collection needs on a volume with the given allocation unit. A
/// file can waste up to one unit, so each is given a whole spare one, plus
/// room for folders and the two manifests.
pub fn space_needed(bytes: u64, files: u64, cluster_bytes: u64) -> u64 {
    let per_file = cluster_bytes.max(1).saturating_add(512);
    bytes.saturating_add(files.saturating_mul(per_file)).saturating_add(4 * 1024 * 1024)
}

/// Copy a selection into a new collection folder and scan the result.
///
/// The staging engine reads every selected file in full, which is when a
/// cloud client downloads the ones it has not fetched, then copies and
/// rehashes them. `destination` must not hold anything else.
pub fn run_collect(
    selection: &Selection,
    destination: &Path,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(&StageProgress),
) -> Result<(Collection, SourceScan), String> {
    let kind = match &selection.source.location {
        Location::Cloud { provider } if provider == "Google Drive" => SourceKind::GoogleDriveDesktop,
        _ => SourceKind::Local,
    };
    let source = selection.source.path.canonicalize().map_err(|_| "The source folder can't be reached.".to_string())?;
    let parent = destination.parent().ok_or("The collection needs a parent folder.")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("The collections folder couldn't be created: {e}"))?;
    let parent = parent.canonicalize().map_err(|e| e.to_string())?;
    let destination = parent.join(destination.file_name().ok_or("The collection needs a name.")?);
    let made = stage_selected(&source, kind, Some(&selection.paths), &destination, cancel, progress).map_err(|e| {
        match e.kind() {
            std::io::ErrorKind::Interrupted => {
                "Stopped. Start again with the same name to pick up where it left off.".to_string()
            }
            std::io::ErrorKind::InvalidInput => format!("The collection couldn't be made: {e}."),
            _ => format!("A file couldn't be read or written: {e}. Start again with the same name to resume."),
        }
    })?;
    let never = std::sync::atomic::AtomicBool::new(false);
    let scan = scan_source(&destination.join("files"), &|_, _| false, &never, |_| {})
        .map_err(|e| format!("The new collection couldn't be read back: {e}"))?;
    Ok((made, scan))
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

fn track_row(source: &str, track: &SourceTrack, state: TrackState, issues: Vec<PlayerIssue>) -> LibraryTrack {
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
    LibraryTrack {
        source: source.to_string(),
        path: track.path.clone(),
        name,
        folder,
        size: track.size,
        format,
        detail,
        state,
        note,
        issues,
    }
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

        let all = library.tracks("demo-computer", &targets, "all", "", "", 0, 500).unwrap();
        assert_eq!(all.total as usize, all.tracks.len());
        assert_eq!(all.total, 13);

        let page = library.tracks("demo-computer", &targets, "all", "", "", 10, 5).unwrap();
        assert_eq!((page.total, page.tracks.len()), (13, 3));

        let blocked = library.tracks("demo-computer", &targets, "blocked", "", "", 0, 500).unwrap();
        assert!(blocked.total > 0);
        assert!(blocked.tracks.iter().all(|t| t.state == TrackState::Blocked && !t.issues.is_empty()));

        let found = library.tracks("demo-computer", &targets, "all", "SUBZERO", "", 0, 500).unwrap();
        assert!(found.total >= 1);
        assert!(found.tracks.iter().all(|t| t.path.to_lowercase().contains("subzero")));

        let online = library.tracks("demo-cloud", &targets, "cloud_only", "", "", 0, 500).unwrap();
        assert_eq!(online.total, 4);
        assert!(online.tracks.iter().all(|t| t.detail.is_empty() && t.issues.is_empty()));

        let broken = library.tracks("demo-computer", &targets, "unreadable", "", "", 0, 500).unwrap();
        assert_eq!(broken.total, 1);
        assert!(broken.tracks[0].note.is_some());

        assert!(library.tracks("missing", &targets, "all", "", "", 0, 10).is_err());
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
    fn collecting_cloud_tracks_makes_a_checked_local_source() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let rules = Ruleset::builtin();
        let targets = club(&rules);
        let mut library = LibraryState::demo(&root).unwrap();

        let picked = library.selection("demo-cloud", &targets, "cloud_only", "", "").unwrap();
        assert_eq!((picked.paths.len(), picked.cloud_only, picked.unreadable), (4, 4, 0));
        assert!(picked.bytes > 0 && picked.bytes == picked.cloud_only_bytes);

        let destination = root.join("Collections").join("Promos");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut phases = Vec::new();
        let (made, scan) = run_collect(&picked, &destination, &cancel, &mut |p| phases.push(p.phase)).unwrap();
        assert_eq!(made.files.len(), 4);
        assert!(phases.contains(&boothready_core::staging::StagePhase::Verifying));
        assert!(destination.join("complete.json").exists());

        let id = library.add_collection(
            CollectionRef {
                name: "Promos".into(),
                root: destination,
                origin: "Promos 2026".into(),
                revision: made.revision,
            },
            scan,
            Location::Computer,
            500,
        );
        let listed = library.list(&targets);
        let source = listed.iter().find(|s| s.id == id).unwrap();
        assert_eq!(source.name, "Promos");
        assert_eq!(source.collected_from.as_deref(), Some("Promos 2026"));
        let states = source.states.unwrap();
        assert_eq!(states.cloud_only, 0, "collected tracks are on this computer and have been checked");
        assert_eq!(states.ready + states.blocked + states.unchecked, 4);
        assert!(states.blocked > 0, "the 24-bit FLAC promo is rejected by the older players once it can be read");
    }

    #[test]
    fn a_selection_leaves_out_unreadable_files_and_follows_the_search() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let targets = club(&rules);
        let mut library = LibraryState::demo(root.path()).unwrap();
        let all = library.selection("demo-computer", &targets, "all", "", "").unwrap();
        assert_eq!((all.paths.len(), all.unreadable), (12, 1));
        let found = library.selection("demo-computer", &targets, "all", "subzero", "").unwrap();
        assert!(
            !found.paths.is_empty()
                && found.paths.iter().all(|p| p.to_string_lossy().to_lowercase().contains("subzero"))
        );
        assert!(library.selection("demo-archive", &targets, "all", "", "").is_ok());
        assert!(library.selection("missing", &targets, "all", "", "").is_err());
    }

    #[test]
    fn collection_names_dates_and_space() {
        assert_eq!(collection_folder_name("  Friday / Panorama: warm-up  ").unwrap(), "Friday - Panorama- warm-up");
        assert_eq!(collection_folder_name("Promos 2026.").unwrap(), "Promos 2026");
        assert!(collection_folder_name("   ").is_err());
        assert!(collection_folder_name("..").is_err());
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(1_790_000_000), "2026-09-21");
        assert_eq!(date_of(951_782_400), "2000-02-29");
        assert_eq!(space_needed(10_000, 3, 4096), 10_000 + 3 * (4096 + 512) + 4 * 1024 * 1024);
        assert_eq!(space_needed(u64::MAX, 3, 4096), u64::MAX);
    }

    #[test]
    fn folders_and_the_whole_collection_can_be_browsed() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let targets = club(&rules);
        let mut library = LibraryState::demo(root.path()).unwrap();

        let folders = library.folders("demo-computer").unwrap();
        let hi_res = folders.iter().find(|f| f.path == "Hi-Res").unwrap();
        assert_eq!((hi_res.depth, hi_res.tracks), (0, 4));
        let speicher = folders.iter().position(|f| f.path == "Kolsch/Speicher").unwrap();
        let kolsch = folders.iter().position(|f| f.path == "Kolsch").unwrap();
        assert_eq!(speicher, kolsch + 1, "a folder is listed right before its subfolders");

        let page = library.tracks("demo-computer", &targets, "all", "", "Hi-Res", 0, 500).unwrap();
        assert_eq!(page.total, 4);
        assert!(page.tracks.iter().all(|t| t.folder == "Hi-Res"));
        assert_eq!(page.states.blocked + page.states.ready, 4);
        let blocked = library.tracks("demo-computer", &targets, "blocked", "", "Hi-Res", 0, 500).unwrap();
        assert_eq!(blocked.states, page.states, "state counts ignore the state filter");
        assert!(library.tracks("demo-computer", &targets, "all", "", "Hi", 0, 500).unwrap().total == 0);

        let everything = library.tracks(ALL_SOURCES, &targets, "all", "", "", 0, 1000).unwrap();
        let separately: u32 = ["demo-computer", "demo-cloud", "demo-archive"]
            .iter()
            .map(|id| library.tracks(id, &targets, "all", "", "", 0, 1000).unwrap().total)
            .sum();
        assert_eq!(everything.total, separately);
        assert!(everything.tracks.iter().any(|t| t.folder.starts_with("Promos 2026/")));

        let picked = library.selection("demo-computer", &targets, "all", "", "Hi-Res").unwrap();
        assert_eq!(picked.paths.len(), 4);
    }

    #[test]
    fn only_audio_inside_a_source_can_be_played() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let mut library = LibraryState::demo(&root).unwrap();
        let file = library.playable_path("demo-computer", "Hi-Res/Ambient Intro.flac").unwrap();
        assert!(file.ends_with("Hi-Res/Ambient Intro.flac") && file.is_file());
        for bad in
            ["../Google Drive/My Drive/Promos 2026/Edits/Opening Tool.mp3", "/etc/hosts", "", "Hi-Res/missing.flac"]
        {
            assert!(library.playable_path("demo-computer", bad).is_err(), "{bad}");
        }
        std::fs::write(root.join("Music/DJ Library/notes.txt"), b"x").unwrap();
        assert!(library.playable_path("demo-computer", "notes.txt").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("Google Drive"), root.join("Music/DJ Library/link.mp3")).unwrap();
            assert!(library.playable_path("demo-computer", "link.mp3").is_err(), "a link out of the source");
        }
        assert!(library.playable_path("missing", "Hi-Res/Ambient Intro.flac").is_err());
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
