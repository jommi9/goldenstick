//! Data sent to the UI. Kept separate from engine types so the UI contract
//! stays small and doesn't ship every track path over IPC.

use boothready_core::drive::{ContentStats, DriveReport, LayoutSource};
use boothready_core::identify::{Candidate, CatalogProduct, Identification};
use boothready_core::library::LibraryFormat;
use boothready_core::planner::Role;
use boothready_core::privileged::{eligibility, DeviceFingerprint, Eligibility};
use boothready_core::rules::DeviceProfile;
use boothready_core::scan::TrackProbe;
use boothready_core::store::KnownMedia;
use boothready_core::verify::VerifyProgress;
use boothready_model::PhysicalDevice;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone)]
pub struct DeviceCard {
    pub device: PhysicalDevice,
    pub identification: Identification,
    pub eligibility: Eligibility,
    pub media_key: String,
    pub known: Option<KnownMedia>,
    pub is_usb: bool,
}

impl DeviceCard {
    pub fn new(
        device: PhysicalDevice,
        identification: Identification,
        media_key: String,
        known: Option<KnownMedia>,
    ) -> Self {
        let eligibility = eligibility(&device);
        let is_usb = device.bus == boothready_model::BusType::Usb;
        DeviceCard { device, identification, eligibility, media_key, known, is_usb }
    }
}

#[derive(Serialize)]
pub struct PlaylistDto {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub is_folder: bool,
    pub tracks: usize,
    pub bytes: u64,
}

#[derive(Serialize)]
pub struct DriveSummary {
    pub device_id: String,
    pub identification: Identification,
    pub eligibility: Eligibility,
    pub scheme: String,
    pub filesystem: Option<String>,
    pub fs_label: Option<String>,
    pub dirty: Option<bool>,
    pub layout_source: LayoutSource,
    pub layout_warnings: Vec<String>,
    pub size_bytes: u64,
    pub mount_point: Option<String>,
    pub content: ContentStats,
    pub formats: Vec<LibraryFormat>,
    pub library_problems: Vec<String>,
    pub playlists: Vec<PlaylistDto>,
    pub library_tracks: usize,
    pub tracks_scanned: usize,
    pub unreadable: usize,
    pub apple_double: u32,
    pub headline: String,
    pub verified: bool,
    pub interrupted: bool,
    pub role: Option<String>,
    pub connection: String,
}

impl DriveSummary {
    pub fn from_report(r: &DriveReport) -> Self {
        let primary_fs = r.layout.primary().and_then(|p| p.filesystem.as_ref());
        let dl = r.libraries.rekordbox.as_ref().and_then(|rb| rb.device_library.as_ref());
        let sizes: HashMap<String, u64> = r.audio.tracks.iter().map(|t| (t.path.clone(), t.size)).collect();
        let playlists = dl
            .map(|dl| {
                let by_id: HashMap<u32, &str> = dl.tracks.iter().map(|t| (t.id, t.path.as_str())).collect();
                dl.playlists
                    .iter()
                    .map(|p| PlaylistDto {
                        id: p.id,
                        parent_id: p.parent_id,
                        name: p.name.clone(),
                        is_folder: p.is_folder,
                        tracks: p.track_ids.len(),
                        bytes: p
                            .track_ids
                            .iter()
                            .filter_map(|id| by_id.get(id))
                            .filter_map(|path| sizes.get(*path))
                            .sum(),
                    })
                    .collect()
            })
            .or_else(|| {
                r.libraries.engine.as_ref().map(|e| {
                    e.playlists
                        .iter()
                        .map(|p| PlaylistDto {
                            id: p.id,
                            parent_id: p.parent_id,
                            name: p.name.clone(),
                            is_folder: p.is_folder,
                            tracks: p.track_ids.len(),
                            bytes: 0,
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        let usb = r.device.usb.as_ref();
        let connection = match (usb.and_then(|u| u.usb_version.as_deref()), usb.and_then(|u| u.speed_mbps)) {
            (_, Some(s)) if s >= 5000 => "USB-A / USB 3.x device".to_string(),
            (_, Some(s)) if s >= 480 => "USB 2.0 device".to_string(),
            (Some(v), _) => format!("USB {v} device"),
            _ => "USB device".to_string(),
        };
        DriveSummary {
            device_id: r.device.id.clone(),
            identification: r.identification.clone(),
            eligibility: r.eligibility.clone(),
            scheme: r.layout.scheme.label().to_string(),
            filesystem: primary_fs.map(|f| f.kind.label().to_string()),
            fs_label: primary_fs
                .and_then(|f| f.label.clone())
                .or_else(|| r.device.primary_volume().and_then(|v| v.label.clone())),
            dirty: primary_fs.and_then(|f| f.dirty),
            layout_source: r.layout_source,
            layout_warnings: r.layout.warnings.clone(),
            size_bytes: r.device.size_bytes,
            mount_point: r.mount_point.clone(),
            content: r.content.clone(),
            formats: r.libraries.formats.clone(),
            library_problems: r.libraries.problems(),
            playlists,
            library_tracks: dl.map(|d| d.tracks.len()).unwrap_or(0),
            tracks_scanned: r.audio.tracks.len(),
            unreadable: r.audio.tracks.iter().filter(|t| matches!(t.probe, TrackProbe::Unreadable(_))).count(),
            apple_double: r.audio.apple_double,
            headline: r.headline.clone(),
            verified: r.verified,
            interrupted: r.interrupted,
            role: r.manifest.as_ref().and_then(|m| m.role).map(|role| role.label().to_string()),
            connection,
        }
    }
}

#[derive(Serialize)]
pub struct HardwareCard {
    pub id: String,
    pub manufacturer: String,
    pub model: String,
    pub family: String,
    pub kind: String,
    pub released: u16,
    pub legacy: bool,
}

impl From<&DeviceProfile> for HardwareCard {
    fn from(d: &DeviceProfile) -> Self {
        HardwareCard {
            id: d.id.clone(),
            manufacturer: d.manufacturer.clone(),
            model: d.model.clone(),
            family: d.family.clone(),
            kind: serde_json::to_value(d.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
            released: d.released,
            legacy: d.legacy,
        }
    }
}

/// Everything the erase confirmation screen shows (PRD §25.3-25.5).
#[derive(Serialize)]
pub struct ConfirmationDetails {
    pub token: String,
    pub display_name: String,
    pub image: String,
    pub color: String,
    pub size_bytes: u64,
    pub volume_label: Option<String>,
    pub serial_tail: Option<String>,
    pub files: u64,
    pub used_bytes: u64,
    pub last_modified_unix: Option<u64>,
    pub eligible: bool,
    pub reasons: Vec<String>,
}

impl ConfirmationDetails {
    pub fn new(r: &DriveReport, fp: &DeviceFingerprint) -> Self {
        ConfirmationDetails {
            token: fp.token(),
            display_name: r.identification.display_name.clone(),
            image: r.identification.image.clone(),
            color: r.identification.color.clone(),
            size_bytes: r.device.size_bytes,
            volume_label: r.device.primary_volume().and_then(|v| v.label.clone()),
            serial_tail: fp.serial_tail(),
            files: r.content.files,
            used_bytes: r.content.used_bytes,
            last_modified_unix: r.content.last_modified_unix,
            eligible: r.eligibility.eligible,
            reasons: r.eligibility.reasons.clone(),
        }
    }
}

pub fn candidate(p: &CatalogProduct, gb: Option<u32>) -> Candidate {
    let size = gb.map(|g| format!(" {g} GB")).unwrap_or_default();
    Candidate {
        catalog_id: p.id.clone(),
        display_name: format!("{} {}{}", p.manufacturer, p.name, size),
        image: p.image.clone(),
        color: p.color.clone(),
    }
}

#[derive(Serialize, Clone)]
pub struct ScanProgressEvent {
    pub device_id: String,
    pub files: u32,
    pub bytes: u64,
    pub current: String,
}

#[derive(Serialize, Clone)]
pub struct PrepareProgress {
    pub step: String,
    pub detail: String,
}

/// A verified drive the app offers to copy from.
#[derive(Serialize)]
pub struct CopySource {
    pub device_id: String,
    pub display_name: String,
    pub role: Option<Role>,
    pub files: u32,
    pub bytes: u64,
    /// Why the copy can't start, in plain words. Empty when it can.
    pub problems: Vec<String>,
    /// The destination holds other files, so erasing it first clears the way.
    pub needs_erase: bool,
}

/// Progress of a verification or a copy, for the drive it concerns.
#[derive(Serialize, Clone)]
pub struct VerifyProgressEvent {
    pub device_id: String,
    pub progress: VerifyProgress,
}

#[derive(Serialize)]
pub struct EjectResult {
    pub ok: bool,
    pub busy_holder: Option<String>,
    pub message: String,
}

#[derive(Serialize)]
pub struct DemoStickInfo {
    pub name: String,
    pub title: String,
    pub size_gb: u64,
    pub inserted: bool,
    pub description: String,
}

#[derive(Serialize)]
pub struct ExportStatus {
    pub device_library: Option<u64>,
    pub one_library: Option<u64>,
    pub engine: Option<u64>,
}
