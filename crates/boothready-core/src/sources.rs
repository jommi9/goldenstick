//! Music sources: a folder on this computer, on an external drive or inside
//! a cloud-synced folder, scanned without changing or downloading anything.
//!
//! A cloud client such as Google Drive for desktop can show a file in a
//! folder while its contents are still online. Opening such a placeholder
//! makes the client download it, so the scan recognises placeholders from
//! their metadata and never opens them. Their format stays unknown until
//! the user downloads them.
//!
//! Scan results hold private file names and belong outside version control.

use crate::audio::tags::TrackTags;
use crate::audio::{is_audio_path, probe_file, ProbeError};
use crate::library::{is_ignored_dir, relative};
use crate::rules::{evaluate_audio, DeviceProfile, TrackVerdict};
use crate::scan::TrackProbe;
use boothready_model::PhysicalDevice;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

/// Whether a file's contents are on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Local,
    /// A cloud placeholder. The name and size are known, the audio is online.
    CloudOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceTrack {
    /// Relative to the source folder, forward slashes.
    pub path: String,
    pub size: u64,
    pub modified_unix: Option<u64>,
    pub availability: Availability,
    /// Header probe. `None` for a cloud-only file, which is never opened.
    pub probe: Option<TrackProbe>,
    /// Artist, title, BPM and key from the file's tags. `None` for a
    /// cloud-only file, an untagged file, or a scan made before tags were read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<TrackTags>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceScan {
    pub tracks: Vec<SourceTrack>,
    /// Folders or entries the walk could not read.
    pub skipped: u32,
    /// The scan was stopped early, so the track list is incomplete.
    pub cancelled: bool,
    /// The scan read the local files' tags. Scans saved by older versions
    /// didn't, so their tracks have no artist or BPM until scanned again.
    #[serde(default)]
    pub tags_read: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTotals {
    pub tracks: u32,
    pub bytes: u64,
    pub cloud_only: u32,
    pub cloud_only_bytes: u64,
    pub unreadable: u32,
}

impl SourceScan {
    pub fn totals(&self) -> SourceTotals {
        let mut t = SourceTotals::default();
        for track in &self.tracks {
            t.tracks += 1;
            t.bytes += track.size;
            if track.availability == Availability::CloudOnly {
                t.cloud_only += 1;
                t.cloud_only_bytes += track.size;
            }
            if matches!(track.probe, Some(TrackProbe::Unreadable(_))) {
                t.unreadable += 1;
            }
        }
        t
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProgress {
    pub files: u32,
    pub bytes: u64,
    pub cloud_only: u32,
    pub current: String,
}

/// True when the operating system marks the file as a cloud placeholder
/// whose contents are not on this computer. Reads metadata only.
pub fn os_placeholder(_path: &Path, meta: &fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        // SF_DATALESS: a File Provider item whose data has not been fetched.
        meta.st_flags() & 0x4000_0000 != 0
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, RECALL_ON_OPEN and OFFLINE.
        meta.file_attributes() & (0x0040_0000 | 0x0004_0000 | 0x1000) != 0
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = meta;
        false
    }
}

/// Walk `root` and describe every audio file under it.
///
/// Local files have their header probed. A file that `placeholder` reports
/// as cloud-only is listed from its metadata and never opened. Symlinks are
/// not followed. Listing a folder that the cloud client has not expanded yet
/// can make the client fetch that folder's file list, which is metadata only.
pub fn scan_source(
    root: &Path,
    placeholder: &dyn Fn(&Path, &fs::Metadata) -> bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&SourceProgress),
) -> io::Result<SourceScan> {
    if !fs::metadata(root)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the music source must be a folder"));
    }
    let mut scan = SourceScan { tags_read: cfg!(feature = "tags"), ..SourceScan::default() };
    let mut p = SourceProgress::default();
    let walk = walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| !is_ignored_dir(e));
    for entry in walk {
        if cancel.load(Ordering::Relaxed) {
            scan.cancelled = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                scan.skipped += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_audio_path(entry.path()) {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with("._") {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => {
                scan.skipped += 1;
                continue;
            }
        };
        let cloud_only = placeholder(entry.path(), &meta);
        let probe = if cloud_only {
            None
        } else {
            Some(match probe_file(entry.path()) {
                Ok(info) => TrackProbe::Ok(info),
                Err(ProbeError::Unrecognized) => TrackProbe::Unreadable("not a recognised audio file".into()),
                Err(e) => TrackProbe::Unreadable(e.to_string()),
            })
        };
        // Tags are read only from a file that is already on this computer
        // and probed as audio, so reading them never downloads anything.
        #[cfg(feature = "tags")]
        let tags = match &probe {
            Some(TrackProbe::Ok(_)) => crate::audio::tags::read_tags(entry.path()),
            _ => None,
        };
        #[cfg(not(feature = "tags"))]
        let tags = None;
        let track = SourceTrack {
            path: relative(root, entry.path()),
            size: meta.len(),
            modified_unix: meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            availability: if cloud_only { Availability::CloudOnly } else { Availability::Local },
            probe,
            tags,
        };
        p.files += 1;
        p.bytes += track.size;
        p.cloud_only += u32::from(cloud_only);
        p.current.clone_from(&track.path);
        progress(&p);
        scan.tracks.push(track);
    }
    scan.tracks.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(scan)
}

/// Where a track stands against a set of players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackState {
    /// On this computer and playable on every selected player.
    Ready,
    /// Still online. Its format can't be checked until it is downloaded.
    CloudOnly,
    /// At least one selected player is documented not to play it.
    Blocked,
    /// No player rejects it, and at least one has no data for this format.
    Unchecked,
    /// The file is damaged or isn't audio.
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerIssue {
    pub device: String,
    pub model: String,
    pub reason: String,
}

/// Judge one track against the selected players. With no players selected,
/// every readable local track is ready.
pub fn judge(track: &SourceTrack, targets: &[&DeviceProfile]) -> (TrackState, Vec<PlayerIssue>) {
    let info = match &track.probe {
        None => return (TrackState::CloudOnly, Vec::new()),
        Some(TrackProbe::Unreadable(_)) => return (TrackState::Unreadable, Vec::new()),
        Some(TrackProbe::Ok(info)) => info,
    };
    let mut blocked = Vec::new();
    let mut unchecked = Vec::new();
    for device in targets {
        let issue = |reason: String| PlayerIssue { device: device.id.clone(), model: device.model.clone(), reason };
        match evaluate_audio(device, info) {
            TrackVerdict::Supported { .. } => {}
            TrackVerdict::Unsupported { reason, .. } => blocked.push(issue(reason)),
            TrackVerdict::Unknown { reason } => unchecked.push(issue(reason)),
        }
    }
    if !blocked.is_empty() {
        (TrackState::Blocked, blocked)
    } else if !unchecked.is_empty() {
        (TrackState::Unchecked, unchecked)
    } else {
        (TrackState::Ready, Vec::new())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateCounts {
    pub ready: u32,
    pub cloud_only: u32,
    pub blocked: u32,
    pub unchecked: u32,
    pub unreadable: u32,
}

impl StateCounts {
    pub fn add(&mut self, state: TrackState) {
        match state {
            TrackState::Ready => self.ready += 1,
            TrackState::CloudOnly => self.cloud_only += 1,
            TrackState::Blocked => self.blocked += 1,
            TrackState::Unchecked => self.unchecked += 1,
            TrackState::Unreadable => self.unreadable += 1,
        }
    }
}

pub fn count_states(scan: &SourceScan, targets: &[&DeviceProfile]) -> StateCounts {
    let mut counts = StateCounts::default();
    for track in &scan.tracks {
        counts.add(judge(track, targets).0);
    }
    counts
}

/// The cloud service a folder appears to be synced by, judged from its path
/// alone. This recognises the folder names the desktop clients create. A
/// renamed or relocated sync folder is reported as an ordinary folder.
pub fn cloud_provider(path: &Path) -> Option<&'static str> {
    let parts: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let parent = if i > 0 { parts[i - 1].as_str() } else { "" };
        if parent == "CloudStorage" {
            // macOS File Provider roots: ~/Library/CloudStorage/<Client>-<account>.
            let client = part.split('-').next().unwrap_or(part);
            return Some(match client {
                "GoogleDrive" => "Google Drive",
                "Dropbox" => "Dropbox",
                "OneDrive" => "OneDrive",
                "Box" => "Box",
                "pCloud" => "pCloud",
                _ => "Cloud storage",
            });
        }
        if parent == "Library" && part == "Mobile Documents" {
            return Some("iCloud Drive");
        }
        let lower = part.to_lowercase();
        if lower == "google drive" || lower == "googledrive" || lower == "my drive" {
            return Some("Google Drive");
        }
        if lower == "dropbox" || lower.starts_with("dropbox (") {
            return Some("Dropbox");
        }
        if lower == "onedrive" || lower.starts_with("onedrive - ") {
            return Some("OneDrive");
        }
        if lower == "icloud drive" || lower == "iclouddrive" {
            return Some("iCloud Drive");
        }
    }
    None
}

/// The storage device a path lives on. Both sides should be canonical paths.
///
/// The device is the one whose mounted volume is the deepest ancestor of the
/// path. Where the two can be compared, the path must also be on that
/// volume's filesystem: macOS reaches the data volume through firmlinks, so
/// a home folder is on `/System/Volumes/Data` without sharing its prefix.
pub fn device_for_path<'a>(path: &Path, devices: &'a [PhysicalDevice]) -> Option<&'a PhysicalDevice> {
    let mut best: Option<(usize, &PhysicalDevice)> = None;
    for device in devices {
        for volume in &device.volumes {
            let Some(mount) = &volume.mount_point else { continue };
            let prefix = path.starts_with(mount);
            let on_volume = same_filesystem(path, mount).unwrap_or(prefix);
            if !on_volume {
                continue;
            }
            let depth = if prefix { mount.components().count() } else { 0 };
            if best.is_none_or(|(d, _)| depth > d) {
                best = Some((depth, device));
            }
        }
    }
    best.map(|(_, device)| device)
}

/// `None` when either path can't be read or the platform has no cheap answer.
fn same_filesystem(a: &Path, b: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(fs::metadata(a).ok()?.dev() == fs::metadata(b).ok()?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        None
    }
}

/// Demo and test stand-in for [`os_placeholder`]: the files a source folder
/// lists, one relative path per line, in `.boothready-demo-cloud` are treated
/// as cloud-only.
#[cfg(any(test, feature = "fixtures"))]
pub fn listed_placeholder(root: &Path) -> impl Fn(&Path, &fs::Metadata) -> bool {
    let listed: std::collections::HashSet<String> = fs::read_to_string(root.join(DEMO_CLOUD_LIST))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let root = root.to_path_buf();
    move |path, _| listed.contains(&relative(&root, path))
}

#[cfg(any(test, feature = "fixtures"))]
pub const DEMO_CLOUD_LIST: &str = ".boothready-demo-cloud";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fixtures as audio;
    use crate::rules::Ruleset;
    use boothready_model::{BusType, Volume};
    use std::path::PathBuf;

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "House/Grey.mp3", &audio::mp3_cbr(120));
        write(root, "House/Subzero.aiff", &audio::aiff(44_100, 24, 2, 22_050, false));
        write(root, "Hi-Res/Ambient.flac", &audio::flac(96_000, 24, 2, 96_000 * 400));
        write(root, "Downloads/Broken.mp3", b"<html>Access denied</html>");
        write(root, "Promos/Online Only.wav", &audio::wav(44_100, 16, 2, 100, false));
        write(root, "House/._Grey.mp3", &[0u8; 4096]);
        write(root, "House/cover.jpg", b"not audio");
        write(root, ".Trashes/Deleted.mp3", &audio::mp3_cbr(10));
        write(root, DEMO_CLOUD_LIST, b"Promos/Online Only.wav\n");
        dir
    }

    fn scan(root: &Path) -> SourceScan {
        scan_source(root, &listed_placeholder(root), &AtomicBool::new(false), |_| {}).unwrap()
    }

    /// Writes a tag the way a DJ tagger would, in the format's own tag type.
    #[cfg(feature = "tags")]
    fn tag(path: &Path, tag_type: lofty::tag::TagType, bpm: &str, key: &str) {
        use lofty::prelude::{Accessor, ItemKey, TagExt};
        let mut tag = lofty::tag::Tag::new(tag_type);
        tag.set_title("Night Drive".to_string());
        tag.set_artist("Test Artist".to_string());
        tag.set_album("Test EP".to_string());
        tag.set_genre("Techno".to_string());
        assert!(tag.insert_text(ItemKey::RecordingDate, "2019-03-01".to_string()));
        // ID3 keeps the tempo in TBPM and MP4 in "tmpo", which lofty calls
        // the integer BPM; Vorbis comments have a free-text BPM field.
        let key_for_bpm =
            if tag_type == lofty::tag::TagType::VorbisComments { ItemKey::Bpm } else { ItemKey::IntegerBpm };
        assert!(tag.insert_text(key_for_bpm, bpm.to_string()));
        assert!(tag.insert_text(ItemKey::InitialKey, key.to_string()));
        tag.save_to_path(path, lofty::config::WriteOptions::default()).unwrap();
    }

    #[cfg(feature = "tags")]
    #[test]
    fn scan_reads_tags_from_local_files_of_every_format() {
        use lofty::tag::TagType;
        let dir = fixture();
        let root = dir.path();
        write(root, "Tagged/Track.wav", &audio::wav(44_100, 16, 2, 22_050, false));
        write(root, "Tagged/Track.m4a", &audio::m4a("aac-lc", 44_100, 30));
        tag(&root.join("House/Grey.mp3"), TagType::Id3v2, "128", "Am");
        tag(&root.join("House/Subzero.aiff"), TagType::Id3v2, "124.5", "8A");
        tag(&root.join("Hi-Res/Ambient.flac"), TagType::VorbisComments, "90", "F#m");
        tag(&root.join("Tagged/Track.wav"), TagType::Id3v2, "174", "1d");
        tag(&root.join("Tagged/Track.m4a"), TagType::Mp4Ilst, "120", "Bbm");
        // The cloud-only file has a tag too, which the scan must not open.
        tag(&root.join("Promos/Online Only.wav"), TagType::Id3v2, "130", "C");
        let scan = scan(root);
        assert!(scan.tags_read);
        let tags = |p: &str| scan.tracks.iter().find(|t| t.path == p).unwrap().tags.clone();
        for (path, bpm, key) in [
            ("House/Grey.mp3", 128.0, "Am"),
            ("House/Subzero.aiff", 124.5, "Am"),
            ("Hi-Res/Ambient.flac", 90.0, "F#m"),
            ("Tagged/Track.wav", 174.0, "C"),
            ("Tagged/Track.m4a", 120.0, "Bbm"),
        ] {
            let t = tags(path).unwrap_or_else(|| panic!("{path} has no tags"));
            assert_eq!(t.title.as_deref(), Some("Night Drive"), "{path}");
            assert_eq!(t.artist.as_deref(), Some("Test Artist"), "{path}");
            assert_eq!(t.album.as_deref(), Some("Test EP"), "{path}");
            assert_eq!(t.genre.as_deref(), Some("Techno"), "{path}");
            assert_eq!(t.year, Some(2019), "{path}");
            assert_eq!(t.bpm, Some(bpm), "{path}");
            assert_eq!(t.key.as_deref(), Some(key), "{path}");
        }
        assert_eq!(tags("Promos/Online Only.wav"), None, "a cloud-only file is never opened");
        assert_eq!(tags("Downloads/Broken.mp3"), None);
    }

    #[test]
    fn scans_saved_before_tags_still_load() {
        let old = r#"{"tracks":[{"path":"a.mp3","size":1,"modified_unix":null,"availability":"local","probe":null}],"skipped":0,"cancelled":false}"#;
        let scan: SourceScan = serde_json::from_str(old).unwrap();
        assert!(!scan.tags_read);
        assert_eq!(scan.tracks[0].tags, None);
    }

    #[test]
    fn lists_audio_and_never_probes_cloud_only_files() {
        let dir = fixture();
        let scan = scan(dir.path());
        let paths: Vec<&str> = scan.tracks.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Downloads/Broken.mp3",
                "Hi-Res/Ambient.flac",
                "House/Grey.mp3",
                "House/Subzero.aiff",
                "Promos/Online Only.wav"
            ]
        );
        let online = &scan.tracks[4];
        assert_eq!(online.availability, Availability::CloudOnly);
        assert_eq!(online.probe, None);
        assert!(online.size > 0);
        assert!(matches!(scan.tracks[0].probe, Some(TrackProbe::Unreadable(_))));
        assert!(matches!(scan.tracks[2].probe, Some(TrackProbe::Ok(_))));
        let totals = scan.totals();
        assert_eq!((totals.tracks, totals.cloud_only, totals.unreadable), (5, 1, 1));
        assert_eq!(totals.cloud_only_bytes, online.size);
        assert!(!scan.cancelled);
    }

    #[test]
    fn progress_counts_every_track_and_cancel_stops_early() {
        let dir = fixture();
        let mut seen = Vec::new();
        scan_source(dir.path(), &listed_placeholder(dir.path()), &AtomicBool::new(false), |p| seen.push(p.clone()))
            .unwrap();
        assert_eq!(seen.len(), 5);
        assert_eq!(seen.last().map(|p| (p.files, p.cloud_only)), Some((5, 1)));

        let stopped = scan_source(dir.path(), &|_, _| false, &AtomicBool::new(true), |_| {}).unwrap();
        assert!(stopped.cancelled);
        assert!(stopped.tracks.is_empty());
    }

    #[test]
    fn a_file_or_missing_folder_is_refused() {
        let dir = fixture();
        let never = |_: &Path, _: &fs::Metadata| false;
        assert!(scan_source(&dir.path().join("House/Grey.mp3"), &never, &AtomicBool::new(false), |_| {}).is_err());
        assert!(scan_source(&dir.path().join("gone"), &never, &AtomicBool::new(false), |_| {}).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_not_followed() {
        let dir = fixture();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "Elsewhere.mp3", &audio::mp3_cbr(10));
        std::os::unix::fs::symlink(outside.path(), dir.path().join("Linked")).unwrap();
        assert!(scan(dir.path()).tracks.iter().all(|t| !t.path.starts_with("Linked")));
    }

    #[test]
    fn ordinary_files_are_not_placeholders() {
        let dir = fixture();
        let path = dir.path().join("House/Grey.mp3");
        assert!(!os_placeholder(&path, &fs::metadata(&path).unwrap()));
    }

    #[test]
    fn tracks_are_judged_against_the_selected_players() {
        let dir = fixture();
        let scan = scan(dir.path());
        let rules = Ruleset::builtin();
        let legacy = rules.device("cdj-2000").unwrap();
        let current = rules.device("cdj-3000").unwrap();
        let by_path = |p: &str| scan.tracks.iter().find(|t| t.path == p).unwrap();

        let (state, issues) = judge(by_path("Hi-Res/Ambient.flac"), &[legacy, current]);
        assert_eq!(state, TrackState::Blocked);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].device, "cdj-2000");

        assert_eq!(judge(by_path("House/Grey.mp3"), &[legacy, current]).0, TrackState::Ready);
        assert_eq!(judge(by_path("Promos/Online Only.wav"), &[legacy]).0, TrackState::CloudOnly);
        assert_eq!(judge(by_path("Downloads/Broken.mp3"), &[legacy]).0, TrackState::Unreadable);
        assert_eq!(judge(by_path("Hi-Res/Ambient.flac"), &[]).0, TrackState::Ready);

        let counts = count_states(&scan, &[legacy, current]);
        assert_eq!((counts.ready, counts.cloud_only, counts.blocked, counts.unreadable), (2, 1, 1, 1));
    }

    #[test]
    fn cloud_folders_are_recognised_from_their_path() {
        let cases = [
            ("/Users/dj/Library/CloudStorage/GoogleDrive-someone@example.com/My Drive/Sets", Some("Google Drive")),
            ("/Users/dj/Library/CloudStorage/Dropbox/Music", Some("Dropbox")),
            ("/Users/dj/Library/CloudStorage/OneDrive-Personal/Music", Some("OneDrive")),
            ("/Users/dj/Library/CloudStorage/SomethingNew-account/Music", Some("Cloud storage")),
            ("/Users/dj/Library/Mobile Documents/com~apple~CloudDocs/Music", Some("iCloud Drive")),
            ("/Users/dj/Dropbox (Studio)/Promos", Some("Dropbox")),
            ("/Users/dj/Music/DJ Library", None),
            ("/Volumes/LaCie/Music", None),
        ];
        for (path, expected) in cases {
            assert_eq!(cloud_provider(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn a_path_belongs_to_the_deepest_mounted_volume() {
        let device = |id: &str, mount: &str, is_system: bool| PhysicalDevice {
            id: id.into(),
            os_path: id.into(),
            bus: if is_system { BusType::Nvme } else { BusType::Usb },
            removable: !is_system,
            is_system,
            size_bytes: 1000,
            logical_sector_size: 512,
            storage_vendor: None,
            storage_model: None,
            storage_revision: None,
            usb: None,
            partition_scheme: None,
            volumes: vec![Volume {
                os_path: id.into(),
                mount_point: Some(PathBuf::from(mount)),
                label: None,
                filesystem: None,
                size_bytes: 1000,
                offset_bytes: None,
                uuid: None,
                efi_system: false,
            }],
        };
        let devices = [device("internal", "/", true), device("external", "/Volumes/LaCie", false)];
        assert_eq!(
            device_for_path(Path::new("/Volumes/LaCie/Music"), &devices).map(|d| d.id.as_str()),
            Some("external")
        );
        assert_eq!(device_for_path(Path::new("/Users/dj/Music"), &devices).map(|d| d.id.as_str()), Some("internal"));
        assert_eq!(device_for_path(Path::new("/Users/dj/Music"), &devices[1..]), None);
    }
}
