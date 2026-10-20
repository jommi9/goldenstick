//! The music library: folders the user added as sources, the last scan of
//! each and how their tracks stand against the selected players.
//!
//! Nothing here writes to a source folder. Records and scans are kept as
//! JSON in the app's own data folder, so a source on an unplugged drive can
//! still be browsed from its last scan.

use boothready_core::audio::tags::{MusicalKey, TrackTags};
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
    /// The last scan read the tags of the tracks on this computer. False for
    /// a scan saved by an older version, which shows file names only.
    pub tags_read: bool,
}

/// One playlist of an export: a folder of the source and the tracks under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlaylist {
    /// Relative to the source, forward slashes.
    pub folder: String,
    /// The folder's own name, which becomes the playlist's name.
    pub name: String,
    /// Relative paths of the tracks, in folder order.
    pub paths: Vec<String>,
}

/// The tracks an export request covers, taken from a source's last scan.
#[derive(Debug, Clone)]
pub struct ExportSelection {
    pub source: SourceRecord,
    pub playlists: Vec<ExportPlaylist>,
    /// Every track once, however many ticked folders hold it.
    pub unique: u32,
    pub bytes: u64,
    pub cloud_only: u32,
    pub cloud_only_bytes: u64,
    /// Tracks left out: damaged, not audio, or a format a Device Library can't hold.
    pub left_out: u32,
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
    /// From the file's tags; the file name stands in when there is no title.
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
    pub bpm: Option<f32>,
    /// Standard notation ("Am") and its Camelot position ("8A").
    pub key: Option<String>,
    pub camelot: Option<String>,
    /// Length in seconds, from the header.
    pub duration: Option<f64>,
}

/// What the browser asks for: a state filter, a search, a folder, an order
/// and a page. Every field has a default, so a caller names only what it needs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TrackQuery {
    /// A [`TrackState`] name, or "all" or empty for every state.
    pub filter: String,
    /// Words that must each appear in the path or the tags.
    pub query: String,
    /// A folder inside the source, or empty for all of it.
    pub folder: String,
    /// A column to order by ("title", "artist", "album", "bpm", "key",
    /// "time", "format", "size" or "state"); empty keeps folder order.
    pub sort: String,
    pub desc: bool,
    pub offset: usize,
    pub limit: usize,
}

impl TrackQuery {
    fn wants(&self, state: TrackState) -> bool {
        self.filter.is_empty() || self.filter == "all" || state_name(state) == self.filter
    }
}

/// The search words, lower-cased. A track matches when every word appears in
/// its path or one of its tags, so "klock subzero" finds artist and album.
fn search_words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

fn matches_search(track: &SourceTrack, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut haystack = track.path.to_lowercase();
    if let Some(tags) = &track.tags {
        for text in [&tags.title, &tags.artist, &tags.album, &tags.genre].into_iter().flatten() {
            haystack.push('\n');
            haystack.push_str(&text.to_lowercase());
        }
    }
    words.iter().all(|w| haystack.contains(w.as_str()))
}

fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Orders two tracks by one column. Tracks missing the value go last in
/// either direction, and ties fall back to folder order.
fn compare_tracks(
    column: &str,
    desc: bool,
    a: (&SourceTrack, TrackState),
    b: (&SourceTrack, TrackState),
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn text(t: &SourceTrack, pick: fn(&TrackTags) -> &Option<String>) -> Option<String> {
        t.tags.as_ref().and_then(|tags| pick(tags).as_ref()).map(|v| v.to_lowercase())
    }
    fn number(v: Option<f64>) -> Option<i64> {
        v.map(|n| (n * 1000.0).round() as i64)
    }
    let title = |t: &SourceTrack| Some(text(t, |g| &g.title).unwrap_or_else(|| file_stem(&t.path).to_lowercase()));
    let duration = |t: &SourceTrack| match &t.probe {
        Some(TrackProbe::Ok(info)) => info.duration_secs,
        _ => None,
    };
    let camelot = |t: &SourceTrack| {
        let key = t.tags.as_ref()?.key.as_deref().and_then(MusicalKey::parse)?;
        let wheel = key.camelot();
        let n: i64 = wheel.trim_end_matches(['A', 'B']).parse().ok()?;
        Some(n * 2 + i64::from(!key.minor))
    };
    let by = |x: Option<String>, y: Option<String>| (x.map(Key::Text), y.map(Key::Text));
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Key {
        Text(String),
        Number(i64),
    }
    let num = |x: Option<i64>, y: Option<i64>| (x.map(Key::Number), y.map(Key::Number));
    let (x, y) = match column {
        "title" => by(title(a.0), title(b.0)),
        "artist" => by(text(a.0, |g| &g.artist), text(b.0, |g| &g.artist)),
        "album" => by(text(a.0, |g| &g.album), text(b.0, |g| &g.album)),
        "bpm" => num(
            number(a.0.tags.as_ref().and_then(|g| g.bpm).map(f64::from)),
            number(b.0.tags.as_ref().and_then(|g| g.bpm).map(f64::from)),
        ),
        "key" => num(camelot(a.0), camelot(b.0)),
        "time" => num(number(duration(a.0)), number(duration(b.0))),
        "size" => num(Some(a.0.size as i64), Some(b.0.size as i64)),
        "format" => by(
            Some(a.0.path.rsplit_once('.').map_or("", |(_, e)| e).to_lowercase()),
            Some(b.0.path.rsplit_once('.').map_or("", |(_, e)| e).to_lowercase()),
        ),
        "state" => num(Some(a.1 as i64), Some(b.1 as i64)),
        _ => (None, None),
    };
    let primary = match (x, y) {
        (Some(x), Some(y)) if desc => y.cmp(&x),
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    // Within one artist, tracks follow their titles.
    let secondary = if column == "artist" { title(a.0).cmp(&title(b.0)) } else { Ordering::Equal };
    primary.then(secondary).then_with(|| a.0.path.cmp(&b.0.path))
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
    exporting: bool,
}

/// The name a source is listed under: its collection name or folder name.
fn display_name(record: &SourceRecord) -> String {
    record.collection.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| name_of(&record.path))
}

/// The name a source is listed under, for records that name their origin.
pub fn name_of_source(record: &SourceRecord) -> String {
    display_name(record)
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
            demo_tags(&mut scan);
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

    /// Every scanned track of every source, for comparing with a stick. Paths
    /// are `<source id>|<path in the source>`, the form the tree's folder
    /// nodes use, so a compared folder maps straight onto its tree node.
    pub fn compare_files(&mut self) -> Vec<boothready_core::library::compare::LibraryFile> {
        let ids: Vec<String> = self.sources.iter().map(|s| s.id.clone()).collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(scan) = self.scan(&id) else { continue };
            for t in &scan.tracks {
                out.push(boothready_core::library::compare::LibraryFile {
                    path: format!("{id}|{}", t.path),
                    size: t.size,
                    title: t.tags.as_ref().and_then(|g| g.title.clone()),
                    artist: t.tags.as_ref().and_then(|g| g.artist.clone()),
                });
            }
        }
        out
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
        let words = search_words(query);
        let wanted = TrackQuery { filter: filter.to_string(), ..TrackQuery::default() };
        let mut picked =
            Selection { source, paths: Vec::new(), bytes: 0, cloud_only: 0, cloud_only_bytes: 0, unreadable: 0 };
        for track in &scan.tracks {
            if !in_folder(&track.path, folder) {
                continue;
            }
            if !matches_search(track, &words) {
                continue;
            }
            let state = judge(track, targets).0;
            if !wanted.wants(state) {
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

    /// A scan, a collect or an export is running. Only one runs at a time.
    pub fn is_busy(&self) -> bool {
        self.collecting || self.exporting || !self.scanning.is_empty()
    }

    pub fn set_collecting(&mut self, on: bool) {
        self.collecting = on;
    }

    pub fn set_exporting(&mut self, on: bool) {
        self.exporting = on;
    }

    /// The tracks an export of the given folders covers: one playlist per
    /// folder, named after it, holding every readable track below it that
    /// a Device Library can store. Cloud-only tracks are included; reading
    /// them makes the cloud client download them.
    pub fn export_selection(&mut self, id: &str, folders: &[String]) -> Result<ExportSelection, String> {
        let source = self.record(id)?;
        let scan = self.scan(id).ok_or_else(|| "Scan this folder first.".to_string())?;
        if folders.is_empty() {
            return Err("Tick at least one folder to export.".to_string());
        }
        let mut picked = ExportSelection {
            source,
            playlists: Vec::new(),
            unique: 0,
            bytes: 0,
            cloud_only: 0,
            cloud_only_bytes: 0,
            left_out: 0,
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut counted_out: HashSet<&str> = HashSet::new();
        for folder in folders {
            let folder = folder.trim_matches('/');
            let relative = Path::new(folder);
            if folder.is_empty() || !relative.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
                return Err("That isn't a folder in this source.".to_string());
            }
            if picked.playlists.iter().any(|p| p.folder == folder) {
                continue;
            }
            let name = folder.rsplit('/').next().unwrap_or(folder).to_string();
            let mut paths = Vec::new();
            for track in scan.tracks.iter().filter(|t| in_folder(&t.path, folder)) {
                let file_name = track.path.rsplit('/').next().unwrap_or(&track.path);
                let usable = !matches!(track.probe, Some(TrackProbe::Unreadable(_)))
                    && boothready_core::library::pdb_writer::file_type_for_path(file_name).is_some();
                if !usable {
                    if counted_out.insert(&track.path) {
                        picked.left_out += 1;
                    }
                    continue;
                }
                paths.push(track.path.clone());
                if seen.insert(&track.path) {
                    picked.unique += 1;
                    picked.bytes += track.size;
                    if track.availability == Availability::CloudOnly {
                        picked.cloud_only += 1;
                        picked.cloud_only_bytes += track.size;
                    }
                }
            }
            if paths.is_empty() && !scan.tracks.iter().any(|t| in_folder(&t.path, folder)) {
                return Err(format!("There is no folder \"{folder}\" in this source."));
            }
            picked.playlists.push(ExportPlaylist { folder: folder.to_string(), name, paths });
        }
        Ok(picked)
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
                let scan = self.scan(&record.id);
                let states = scan.as_ref().map(|scan| count_states(scan, targets));
                let tags_read = scan.as_ref().is_none_or(|scan| {
                    scan.tags_read || scan.tracks.iter().all(|t| t.availability == Availability::CloudOnly)
                });
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
                    tags_read,
                    id: record.id,
                }
            })
            .collect()
    }

    /// One page of a source's tracks, filtered by folder, by a search on the
    /// path and tags and by state, in the order the query asks for. `id` may
    /// be [`ALL_SOURCES`], which pages through every source and puts each
    /// source's name in front of the folder.
    pub fn tracks(&mut self, id: &str, targets: &[&DeviceProfile], q: &TrackQuery) -> Result<LibraryPage, String> {
        let ids: Vec<String> = if id == ALL_SOURCES {
            self.sources.iter().map(|s| s.id.clone()).collect()
        } else {
            self.record(id)?;
            vec![id.to_string()]
        };
        let words = search_words(&q.query);
        let folder = q.folder.trim_matches('/');
        let mut scans = Vec::new();
        for sid in &ids {
            if let Some(scan) = self.scan(sid) {
                let label = if id == ALL_SOURCES { self.record(sid).ok().map(|r| display_name(&r)) } else { None };
                scans.push((sid.clone(), label, scan));
            }
        }
        let mut states = StateCounts::default();
        let mut picked: Vec<(usize, &SourceTrack, TrackState)> = Vec::new();
        for (n, (_, _, scan)) in scans.iter().enumerate() {
            for track in &scan.tracks {
                if !in_folder(&track.path, folder) || !matches_search(track, &words) {
                    continue;
                }
                let state = judge(track, targets).0;
                states.add(state);
                if q.wants(state) {
                    picked.push((n, track, state));
                }
            }
        }
        if !q.sort.is_empty() {
            picked.sort_by(|a, b| compare_tracks(&q.sort, q.desc, (a.1, a.2), (b.1, b.2)));
        }
        let tracks = picked
            .iter()
            .skip(q.offset)
            .take(q.limit)
            .map(|&(n, track, state)| {
                let (sid, label, _) = &scans[n];
                let mut row = track_row(sid, track, state, judge(track, targets).1);
                if let Some(source) = label {
                    row.folder =
                        if row.folder.is_empty() { source.clone() } else { format!("{source}/{}", row.folder) };
                }
                row
            })
            .collect();
        Ok(LibraryPage { total: picked.len() as u32, tracks, states })
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

/// Demo files are a few kilobytes of valid headers. Give them the sizes and
/// lengths real tracks have, so the simulated library reads like a real one.
/// A damaged file keeps its true size.
pub(crate) fn demo_sizes(scan: &mut SourceScan) {
    for track in &mut scan.tracks {
        let lossless = match &track.probe {
            Some(TrackProbe::Ok(info)) => is_lossless(info),
            Some(TrackProbe::Unreadable(_)) => continue,
            None => [".wav", ".aiff", ".aif", ".flac"].iter().any(|e| track.path.to_lowercase().ends_with(e)),
        };
        // FNV-1a, so a track keeps the same size on every run.
        let hash = demo_hash(&track.path);
        track.size = if lossless { 38_000_000 + hash % 40_000_000 } else { 7_000_000 + hash % 8_000_000 };
        if let Some(TrackProbe::Ok(info)) = &mut track.probe {
            info.duration_secs = Some(270.0 + (hash % 240) as f64);
        }
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
            tags: None,
        });
    }
    scan.tracks.sort_by(|a, b| a.path.cmp(&b.path));
    demo_sizes(&mut scan);
    demo_tags(&mut scan);
    scan
}

/// FNV-1a of a path, so a demo track gets the same made-up values every run.
fn demo_hash(path: &str) -> u64 {
    path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The demo files are bare headers with no tags. Give the readable ones the
/// tags a DJ's files would have: artist and album from "Artist/Album/Title"
/// folders, and a tempo and key that suit the crate. One track is left
/// untagged so the file name fallback shows.
pub(crate) fn demo_tags(scan: &mut SourceScan) {
    scan.tags_read = true;
    const KEYS: [&str; 24] = [
        "Am", "Em", "Bm", "F#m", "C#m", "G#m", "Ebm", "Bbm", "Fm", "Cm", "Gm", "Dm", "C", "G", "D", "A", "E", "B",
        "F#", "Db", "Ab", "Eb", "Bb", "F",
    ];
    for track in &mut scan.tracks {
        if !matches!(track.probe, Some(TrackProbe::Ok(_))) || track.path.ends_with("Studio Master.wav") {
            continue;
        }
        let hash = demo_hash(&track.path);
        let parts: Vec<&str> = track.path.split('/').collect();
        let (low, span) = match parts[0] {
            "Ambient" => (82, 16),
            "Disco" => (114, 8),
            "House" | "Edits" => (120, 7),
            "Breaks" => (128, 8),
            "Hi-Res" => (118, 10),
            _ => (128, 12),
        };
        let archive = file_stem(&track.path).starts_with("Archive Track");
        track.tags = Some(TrackTags {
            title: (!archive).then(|| file_stem(&track.path).to_string()),
            artist: (parts.len() == 3 && !archive).then(|| parts[0].to_string()),
            album: (parts.len() == 3 && !archive).then(|| parts[1].to_string()),
            genre: Some(if archive { parts[0] } else { "Techno" }.to_string()),
            year: Some(2012 + (hash % 14) as u16),
            bpm: Some((low + (hash % span)) as f32 + if hash % 5 == 0 { 0.5 } else { 0.0 }),
            key: Some(KEYS[(hash >> 8) as usize % KEYS.len()].to_string()),
        });
    }
}

fn is_lossless(info: &AudioInfo) -> bool {
    matches!(info.container, Container::Wav | Container::Aiff | Container::Flac) || info.codec == Codec::Alac
}

/// A folder name for a collection, made from what the user typed. Anything
/// outside letters, digits, spaces and a few marks becomes a dash, so the
/// name is valid on FAT, NTFS and APFS alike.
/// Copy `files` (paths under `from_root`) into the folder `target`, creating
/// it. A file already there with the same size is skipped and one with another
/// size is kept beside it as "<name> (from stick)"; nothing is overwritten.
/// Returns how many files were copied.
pub fn copy_into(from_root: &Path, files: &[String], target: &Path) -> Result<u32, String> {
    std::fs::create_dir_all(target).map_err(|e| format!("Couldn't make {}: {e}", target.display()))?;
    let mut copied = 0;
    for rel in files {
        let from = from_root.join(rel);
        let Some(name) = from.file_name().map(|n| n.to_owned()) else { continue };
        let Ok(meta) = std::fs::metadata(&from) else { continue };
        let mut to = target.join(&name);
        if let Ok(existing) = std::fs::metadata(&to) {
            if existing.len() == meta.len() {
                continue;
            }
            let stem = to.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = to.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            to = target.join(format!("{stem} (from stick){ext}"));
            if to.exists() {
                continue;
            }
        }
        std::fs::copy(&from, &to).map_err(|e| format!("Couldn't copy {}: {e}", name.to_string_lossy()))?;
        copied += 1;
    }
    Ok(copied)
}

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
    let tags = track.tags.as_ref();
    let key = tags.and_then(|t| t.key.as_deref()).and_then(MusicalKey::parse);
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
        title: tags.and_then(|t| t.title.clone()),
        artist: tags.and_then(|t| t.artist.clone()),
        album: tags.and_then(|t| t.album.clone()),
        genre: tags.and_then(|t| t.genre.clone()),
        year: tags.and_then(|t| t.year),
        bpm: tags.and_then(|t| t.bpm),
        camelot: key.map(MusicalKey::camelot),
        key: key.map(|k| k.name().to_string()),
        duration: match &track.probe {
            Some(TrackProbe::Ok(info)) => info.duration_secs,
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn copying_from_a_stick_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let stick = dir.path().join("stick");
        std::fs::create_dir_all(stick.join("Contents/A")).unwrap();
        std::fs::write(stick.join("Contents/A/one.mp3"), b"one").unwrap();
        std::fs::write(stick.join("Contents/A/two.mp3"), b"two-on-stick").unwrap();
        std::fs::write(stick.join("Contents/A/three.mp3"), b"333").unwrap();
        let target = dir.path().join("Music/DJ/acid prog");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("one.mp3"), b"one").unwrap();
        std::fs::write(target.join("two.mp3"), b"two-in-the-drive").unwrap();
        let files = ["Contents/A/one.mp3", "Contents/A/two.mp3", "Contents/A/three.mp3", "Contents/A/gone.mp3"]
            .map(String::from);
        assert_eq!(super::copy_into(&stick, &files, &target).unwrap(), 2);
        assert_eq!(
            std::fs::read(target.join("two.mp3")).unwrap(),
            b"two-in-the-drive",
            "an existing file is never overwritten"
        );
        assert_eq!(std::fs::read(target.join("two (from stick).mp3")).unwrap(), b"two-on-stick");
        assert_eq!(std::fs::read(target.join("three.mp3")).unwrap(), b"333");
        assert_eq!(super::copy_into(&stick, &files, &target).unwrap(), 0, "a second run copies nothing");
    }

    use super::*;
    use boothready_core::rules::Ruleset;

    fn ask(filter: &str, query: &str, folder: &str, offset: usize, limit: usize) -> TrackQuery {
        TrackQuery {
            filter: filter.into(),
            query: query.into(),
            folder: folder.into(),
            offset,
            limit,
            ..TrackQuery::default()
        }
    }

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
    fn tags_are_searched_and_sorted() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let targets = club(&rules);
        let mut library = LibraryState::demo(root.path()).unwrap();
        assert!(library.list(&targets).iter().all(|s| s.tags_read));

        // Every word must match somewhere: the artist folder and the album tag.
        let found = library.tracks("demo-computer", &targets, &ask("all", "klock subzero", "", 0, 50)).unwrap();
        assert_eq!(found.total, 2);
        assert!(found.tracks.iter().all(|t| t.artist.as_deref() == Some("Ben Klock")));
        let first = &found.tracks[0];
        assert!(first.title.is_some() && first.bpm.is_some() && first.camelot.is_some() && first.duration.is_some());

        let mut sorted = |sort: &str, desc: bool| {
            let q = TrackQuery { sort: sort.into(), desc, limit: 500, ..TrackQuery::default() };
            library.tracks("demo-computer", &targets, &q).unwrap().tracks
        };
        for desc in [false, true] {
            let rows = sorted("bpm", desc);
            let tempos: Vec<f32> = rows.iter().filter_map(|t| t.bpm).collect();
            let mut expected = tempos.clone();
            expected.sort_by(|a, b| if desc { b.total_cmp(a) } else { a.total_cmp(b) });
            assert_eq!(tempos, expected);
            // Untagged and unreadable tracks go last in both directions.
            let first_missing = rows.iter().position(|t| t.bpm.is_none()).unwrap();
            assert!(rows[first_missing..].iter().all(|t| t.bpm.is_none()));
        }
        let wheel: Vec<String> = sorted("key", false).iter().filter_map(|t| t.camelot.clone()).collect();
        let position = |c: &String| {
            let n: u32 = c.trim_end_matches(['A', 'B']).parse().unwrap();
            n * 2 + u32::from(c.ends_with('B'))
        };
        assert!(wheel.windows(2).all(|w| position(&w[0]) <= position(&w[1])), "{wheel:?}");
        let artists: Vec<Option<String>> = sorted("artist", false).iter().map(|t| t.artist.clone()).collect();
        let named = artists.iter().take_while(|a| a.is_some()).count();
        assert!(artists[..named].windows(2).all(|w| w[0] <= w[1]));
        assert!(artists[named..].iter().all(Option::is_none));
        // The file name stands in for a missing title when sorting by title.
        let titles: Vec<String> =
            sorted("title", false).iter().map(|t| t.title.clone().unwrap_or_else(|| t.name.clone())).collect();
        assert!(titles.windows(2).all(|w| w[0].to_lowercase() <= w[1].to_lowercase()), "{titles:?}");
    }

    #[test]
    fn a_scan_from_before_tags_asks_to_be_scanned_again() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let mut library = LibraryState::demo(root.path()).unwrap();
        let mut old = (*library.scans["demo-computer"]).clone();
        old.tags_read = false;
        for track in &mut old.tracks {
            track.tags = None;
        }
        library.scans.insert("demo-computer".into(), Arc::new(old));
        let sources = library.list(&club(&rules));
        assert!(!sources[0].tags_read);
        // The cloud folder's scan read tags; its online files have none to read.
        assert!(sources[1].tags_read);
    }

    #[test]
    fn tracks_are_filtered_searched_and_paged() {
        let root = tempfile::tempdir().unwrap();
        let rules = Ruleset::builtin();
        let targets = club(&rules);
        let mut library = LibraryState::demo(root.path()).unwrap();

        let all = library.tracks("demo-computer", &targets, &ask("all", "", "", 0, 500)).unwrap();
        assert_eq!(all.total as usize, all.tracks.len());
        assert_eq!(all.total, 13);

        let page = library.tracks("demo-computer", &targets, &ask("all", "", "", 10, 5)).unwrap();
        assert_eq!((page.total, page.tracks.len()), (13, 3));

        let blocked = library.tracks("demo-computer", &targets, &ask("blocked", "", "", 0, 500)).unwrap();
        assert!(blocked.total > 0);
        assert!(blocked.tracks.iter().all(|t| t.state == TrackState::Blocked && !t.issues.is_empty()));

        let found = library.tracks("demo-computer", &targets, &ask("all", "SUBZERO", "", 0, 500)).unwrap();
        assert!(found.total >= 1);
        assert!(found.tracks.iter().all(|t| t.path.to_lowercase().contains("subzero")));

        let online = library.tracks("demo-cloud", &targets, &ask("cloud_only", "", "", 0, 500)).unwrap();
        assert_eq!(online.total, 4);
        assert!(online.tracks.iter().all(|t| t.detail.is_empty() && t.issues.is_empty()));

        let broken = library.tracks("demo-computer", &targets, &ask("unreadable", "", "", 0, 500)).unwrap();
        assert_eq!(broken.total, 1);
        assert!(broken.tracks[0].note.is_some());

        assert!(library.tracks("missing", &targets, &ask("all", "", "", 0, 10)).is_err());
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
    fn an_export_selection_is_one_playlist_per_folder() {
        let root = tempfile::tempdir().unwrap();
        let mut library = LibraryState::demo(root.path()).unwrap();
        let picked = library.export_selection("demo-computer", &["Hi-Res".into(), "Kolsch/Speicher".into()]).unwrap();
        assert_eq!(picked.playlists.len(), 2);
        assert_eq!(picked.playlists[0].name, "Hi-Res");
        assert_eq!(picked.playlists[0].paths.len(), 4);
        assert_eq!(picked.playlists[1].name, "Speicher");
        assert!(picked.playlists[1].paths.iter().all(|p| p.starts_with("Kolsch/Speicher/")));
        assert_eq!(picked.unique, picked.playlists.iter().map(|p| p.paths.len() as u32).sum::<u32>());
        assert!(picked.bytes > 0);
        // A folder and one inside it count their shared tracks once.
        let nested = library.export_selection("demo-computer", &["Kolsch".into(), "Kolsch/Speicher".into()]).unwrap();
        assert_eq!(nested.unique, nested.playlists[0].paths.len() as u32);
        // The damaged download is left out of the whole source.
        let all = library.export_selection("demo-computer", &["Downloads".into()]).unwrap();
        assert_eq!((all.unique, all.left_out), (0, 1));
        let online = library.export_selection("demo-cloud", &["Label Promos".into()]).unwrap();
        assert_eq!(online.cloud_only, 4);
        assert!(library.export_selection("demo-computer", &[]).is_err());
        assert!(library.export_selection("demo-computer", &["../Google Drive".into()]).is_err());
        assert!(library.export_selection("demo-computer", &["Nope".into()]).is_err());
        assert!(library.export_selection("missing", &["Hi-Res".into()]).is_err());
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

        let page = library.tracks("demo-computer", &targets, &ask("all", "", "Hi-Res", 0, 500)).unwrap();
        assert_eq!(page.total, 4);
        assert!(page.tracks.iter().all(|t| t.folder == "Hi-Res"));
        assert_eq!(page.states.blocked + page.states.ready, 4);
        let blocked = library.tracks("demo-computer", &targets, &ask("blocked", "", "Hi-Res", 0, 500)).unwrap();
        assert_eq!(blocked.states, page.states, "state counts ignore the state filter");
        assert!(library.tracks("demo-computer", &targets, &ask("all", "", "Hi", 0, 500)).unwrap().total == 0);

        let everything = library.tracks(ALL_SOURCES, &targets, &ask("all", "", "", 0, 1000)).unwrap();
        let separately: u32 = ["demo-computer", "demo-cloud", "demo-archive"]
            .iter()
            .map(|id| library.tracks(id, &targets, &ask("all", "", "", 0, 1000)).unwrap().total)
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
