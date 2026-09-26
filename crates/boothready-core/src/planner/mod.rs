//! Multi-USB kit planning and capacity fitting.
//!
//! A kit treats several USB drives as one system: each gets a role, a layout
//! and a content policy chosen so that together they cover the most
//! equipment, with a reason for every choice.

use crate::library::LibraryFormat;
use crate::rules::{recommend_format, DeviceProfile, Evidence, FormatRecommendation, Ruleset, Support};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const GB: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Main,
    LegacyRescue,
    Backup,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Main => "Main",
            Role::LegacyRescue => "Legacy Rescue",
            Role::Backup => "Independent Backup",
        }
    }

    /// Volume label written to the drive. FAT limits labels to 11 characters.
    pub fn volume_label(self) -> &'static str {
        match self {
            Role::Main => "BR_MAIN",
            Role::LegacyRescue => "BR_LEGACY",
            Role::Backup => "BR_BACKUP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPolicy {
    FullLibrary,
    EssentialPlaylists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioPolicy {
    /// Copy tracks as they are.
    AsIs,
    /// Leave out tracks the role's players can't play.
    TargetSafeOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveCandidate {
    pub id: String,
    pub name: String,
    pub vendor: Option<String>,
    pub capacity_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitRequest {
    pub targets: Vec<String>,
    pub redundancy: bool,
    /// Size of the full library the Main drive should hold.
    pub library_bytes: u64,
    /// Size of the playlists marked essential, if the user picked any.
    pub essential_bytes: Option<u64>,
    pub drives: Vec<DriveCandidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RolePlan {
    pub role: Role,
    pub volume_label: String,
    pub targets: Vec<String>,
    pub target_models: Vec<String>,
    pub format: FormatRecommendation,
    pub library_formats: Vec<LibraryFormat>,
    pub content: ContentPolicy,
    pub audio: AudioPolicy,
    pub min_capacity_bytes: u64,
    /// Drive sizes that suit this role, smallest to largest.
    pub ideal_capacity: (u64, u64),
    pub purpose: Vec<String>,
    pub why: String,
    pub assigned_drive: Option<String>,
    pub drive_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KitPlan {
    pub roles: Vec<RolePlan>,
    /// Targets no role can serve with known-good data.
    pub not_covered: Vec<String>,
    pub notes: Vec<String>,
}

const STANDARD_SIZES: [u64; 9] = [4, 8, 16, 32, 64, 128, 256, 512, 1024];

/// Space needed for `bytes` of music plus analysis files and filesystem
/// overhead. rekordbox analysis data runs at a few percent of the audio.
pub fn with_overhead(bytes: u64) -> u64 {
    bytes + bytes / 12 + GB / 2
}

/// Usable bytes on a drive sold as `n` GB is a little under n × 10^9.
fn smallest_standard_size(bytes: u64) -> u64 {
    for s in STANDARD_SIZES {
        if s * GB * 93 / 100 >= bytes {
            return s * GB;
        }
    }
    bytes.div_ceil(GB) * GB
}

fn fmt_gb(bytes: u64) -> String {
    let gb = bytes as f64 / GB as f64;
    if gb >= 10.0 || gb.fract() == 0.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

/// Smallest set of library formats such that every target reads at least one.
pub fn required_library_formats(targets: &[&DeviceProfile]) -> (Vec<LibraryFormat>, Vec<String>) {
    let readable: Vec<(String, HashSet<LibraryFormat>)> =
        targets.iter().map(|d| (d.id.clone(), d.readable_libraries().into_iter().map(|(f, _)| f).collect())).collect();
    let mut uncovered: HashSet<String> =
        readable.iter().filter(|(_, s)| !s.is_empty()).map(|(id, _)| id.clone()).collect();
    let not_covered: Vec<String> = readable.iter().filter(|(_, s)| s.is_empty()).map(|(id, _)| id.clone()).collect();
    let mut chosen = Vec::new();
    while !uncovered.is_empty() {
        let mut counts: BTreeMap<LibraryFormat, usize> = BTreeMap::new();
        for (id, set) in &readable {
            if uncovered.contains(id) {
                for f in set {
                    *counts.entry(*f).or_default() += 1;
                }
            }
        }
        // BTreeMap order breaks ties towards the older, wider-supported format.
        let Some((&best, _)) = counts.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) else { break };
        chosen.push(best);
        uncovered.retain(|id| !readable.iter().any(|(rid, set)| rid == id && set.contains(&best)));
    }
    chosen.sort();
    (chosen, not_covered)
}

fn models(devs: &[&DeviceProfile]) -> Vec<String> {
    devs.iter().map(|d| d.model.clone()).collect()
}

fn join(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

fn role_plan(
    role: Role,
    devs: &[&DeviceProfile],
    content: ContentPolicy,
    audio: AudioPolicy,
    min: u64,
    ideal: (u64, u64),
) -> RolePlan {
    let format = recommend_format(devs);
    let (library_formats, _) = required_library_formats(devs);
    let names = models(devs);
    let lib_names: Vec<String> = library_formats.iter().map(|f| f.label().to_string()).collect();
    let (purpose, why) = match role {
        Role::Main => (
            vec![
                "Your complete library".to_string(),
                format!("Current hardware: {}", join(&names)),
                format!("{} + {} with {}", format.scheme.label(), format.filesystem.label(), join(&lib_names)),
            ],
            format!(
                "This is the drive you plug in first. It holds everything and is set up for the {}. {}",
                join(&names),
                format.why
            ),
        ),
        Role::LegacyRescue => (
            vec![
                "Essential playlists only".to_string(),
                format!("Conservative {} + {} setup", format.scheme.label(), format.filesystem.label()),
                "Only tracks older players can play".to_string(),
                format!("Older players: {}", join(&names)),
            ],
            format!(
                "If the booth has older players like the {}, this drive is the one that works. It skips files they can't play and keeps the library small, because older players browse large libraries slowly.",
                join(&names)
            ),
        ),
        Role::Backup => (
            vec![
                "A second copy of your Main USB".to_string(),
                "Ideally a different brand from Main".to_string(),
            ],
            "USB drives fail. A backup from a different manufacturer means one bad batch of flash memory can't take out both drives.".to_string(),
        ),
    };
    RolePlan {
        role,
        volume_label: role.volume_label().to_string(),
        targets: devs.iter().map(|d| d.id.clone()).collect(),
        target_models: names,
        format,
        library_formats,
        content,
        audio,
        min_capacity_bytes: min,
        ideal_capacity: ideal,
        purpose,
        why,
        assigned_drive: None,
        drive_notes: vec![],
    }
}

pub fn plan_kit(req: &KitRequest, rules: &Ruleset) -> KitPlan {
    let targets = rules.devices_by_id(&req.targets);
    let current: Vec<&DeviceProfile> = targets.iter().copied().filter(|d| !d.legacy).collect();
    let legacy: Vec<&DeviceProfile> = targets.iter().copied().filter(|d| d.legacy).collect();
    let mut notes = Vec::new();

    let main_min = with_overhead(req.library_bytes);
    let essential = req.essential_bytes.unwrap_or(req.library_bytes.min(24 * GB));
    let legacy_min = with_overhead(essential);

    let mut roles = Vec::new();
    let main_devs = if current.is_empty() { legacy.clone() } else { current.clone() };
    let main_audio = if current.is_empty() { AudioPolicy::TargetSafeOnly } else { AudioPolicy::AsIs };
    roles.push(role_plan(
        Role::Main,
        &main_devs,
        ContentPolicy::FullLibrary,
        main_audio,
        main_min,
        (smallest_standard_size(main_min), smallest_standard_size(main_min).max(128 * GB)),
    ));
    if !current.is_empty() && !legacy.is_empty() {
        let small = smallest_standard_size(legacy_min);
        roles.push(role_plan(
            Role::LegacyRescue,
            &legacy,
            ContentPolicy::EssentialPlaylists,
            AudioPolicy::TargetSafeOnly,
            legacy_min,
            (small.max(8 * GB), small.max(32 * GB)),
        ));
        notes.push(
            "Keeping the Legacy Rescue drive between 8 and 32 GB is a precaution for older players, not a documented limit.".into(),
        );
    }
    if req.redundancy {
        roles.push(role_plan(
            Role::Backup,
            &main_devs,
            ContentPolicy::FullLibrary,
            AudioPolicy::AsIs,
            main_min,
            (smallest_standard_size(main_min), smallest_standard_size(main_min).max(128 * GB)),
        ));
    }

    let mut not_covered: Vec<String> = Vec::new();
    for r in &roles {
        not_covered.extend(r.format.not_covered.iter().cloned());
    }
    let (_, no_lib) = required_library_formats(&targets);
    not_covered.extend(no_lib);
    not_covered.sort();
    not_covered.dedup();
    // A device is only "not covered" if no role serves it.
    not_covered.retain(|id| !roles.iter().any(|r| r.targets.contains(id) && !r.format.not_covered.contains(id)));

    assign_drives(&mut roles, &req.drives, legacy_min);
    if targets
        .iter()
        .any(|d| d.library_formats.values().any(|c| c.support == Support::Supported && c.evidence < Evidence::Vendor))
    {
        notes.push("Some library support below is inferred from related models rather than documented.".into());
    }
    KitPlan { roles, not_covered, notes }
}

fn assign_drives(roles: &mut [RolePlan], drives: &[DriveCandidate], essential_min: u64) {
    let mut free: Vec<&DriveCandidate> = drives.iter().collect();
    free.sort_by(|a, b| b.capacity_bytes.cmp(&a.capacity_bytes));
    let fits = |d: &DriveCandidate, min: u64| d.capacity_bytes * 93 / 100 >= min;
    let mut main_vendor: Option<String> = None;
    for role in [Role::Main, Role::LegacyRescue, Role::Backup] {
        let Some(plan) = roles.iter_mut().find(|r| r.role == role) else { continue };
        let pick = match role {
            Role::Main => free.iter().position(|d| fits(d, plan.min_capacity_bytes)),
            Role::LegacyRescue => {
                // Smallest drive that fits, preferring ones inside the ideal range.
                let mut idx: Vec<usize> = (0..free.len()).filter(|&i| fits(free[i], plan.min_capacity_bytes)).collect();
                idx.sort_by_key(|&i| (free[i].capacity_bytes > plan.ideal_capacity.1, free[i].capacity_bytes));
                idx.first().copied()
            }
            Role::Backup => {
                let pick_for = |min: u64| {
                    let candidates: Vec<usize> = (0..free.len()).filter(|&i| fits(free[i], min)).collect();
                    candidates
                        .iter()
                        .copied()
                        .find(|&i| {
                            free[i].vendor.as_deref().map(str::to_lowercase)
                                != main_vendor.as_deref().map(str::to_lowercase)
                        })
                        .or_else(|| candidates.first().copied())
                };
                match pick_for(plan.min_capacity_bytes) {
                    Some(i) => Some(i),
                    None => {
                        // Too small for everything: back up the essentials instead.
                        let i = pick_for(essential_min);
                        if i.is_some() {
                            plan.content = ContentPolicy::EssentialPlaylists;
                            plan.min_capacity_bytes = essential_min;
                            plan.purpose[0] = "A copy of your essential playlists".into();
                            plan.drive_notes
                                .push("Too small for your whole library, so it holds your essential playlists.".into());
                        }
                        i
                    }
                }
            }
        };
        match pick {
            Some(i) => {
                let d = free.remove(i);
                if role == Role::Main {
                    main_vendor = d.vendor.clone();
                }
                if role == Role::Backup
                    && d.vendor.is_some()
                    && d.vendor.as_deref().map(str::to_lowercase) == main_vendor.as_deref().map(str::to_lowercase)
                {
                    plan.drive_notes
                        .push("Same brand as Main. A different brand would protect you against a bad batch.".into());
                }
                if role == Role::LegacyRescue && d.capacity_bytes > plan.ideal_capacity.1 {
                    plan.drive_notes.push(format!(
                        "{} is larger than ideal for older players; it should still work.",
                        fmt_gb(d.capacity_bytes)
                    ));
                }
                plan.assigned_drive = Some(d.id.clone());
            }
            None => {
                plan.drive_notes.push(format!(
                    "Insert a USB of at least {}.",
                    fmt_gb(smallest_standard_size(plan.min_capacity_bytes))
                ));
                if let Some(biggest) = free.first() {
                    if !fits(biggest, plan.min_capacity_bytes) {
                        plan.drive_notes.push(format!(
                            "{} ({}) is too small: this role needs about {}.",
                            biggest.name,
                            fmt_gb(biggest.capacity_bytes),
                            fmt_gb(plan.min_capacity_bytes)
                        ));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSize {
    pub id: u32,
    pub name: String,
    pub track_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FitResult {
    pub included_playlists: Vec<u32>,
    pub excluded_playlists: Vec<u32>,
    pub included_tracks: Vec<u32>,
    /// Tracks left out because the role's players can't play them.
    pub skipped_incompatible: Vec<u32>,
    pub bytes: u64,
    pub capacity_bytes: u64,
}

/// Fit playlists onto a drive in priority order. A track shared by several
/// playlists is counted once. Playlists that don't fit are skipped, and
/// smaller ones after them still get a chance. Everything left out is
/// reported; nothing is dropped silently.
pub fn fit_playlists(
    playlists: &[PlaylistSize],
    track_bytes: &HashMap<u32, u64>,
    capacity_bytes: u64,
    include_track: impl Fn(u32) -> bool,
) -> FitResult {
    let budget = capacity_bytes.saturating_sub(capacity_bytes / 12 + GB / 2);
    let mut chosen: HashSet<u32> = HashSet::new();
    let mut skipped: HashSet<u32> = HashSet::new();
    let mut r = FitResult {
        included_playlists: vec![],
        excluded_playlists: vec![],
        included_tracks: vec![],
        skipped_incompatible: vec![],
        bytes: 0,
        capacity_bytes,
    };
    for p in playlists {
        let new: Vec<u32> = p
            .track_ids
            .iter()
            .copied()
            .filter(|t| !chosen.contains(t))
            .filter(|t| {
                let ok = include_track(*t);
                if !ok {
                    skipped.insert(*t);
                }
                ok
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let add: u64 = new.iter().map(|t| track_bytes.get(t).copied().unwrap_or(0)).sum();
        if r.bytes + add <= budget {
            r.bytes += add;
            for t in &new {
                chosen.insert(*t);
            }
            r.included_playlists.push(p.id);
        } else {
            r.excluded_playlists.push(p.id);
        }
    }
    r.included_tracks = chosen.into_iter().collect();
    r.included_tracks.sort_unstable();
    r.skipped_incompatible = skipped.into_iter().collect();
    r.skipped_incompatible.sort_unstable();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(id: &str, vendor: &str, gb: u64) -> DriveCandidate {
        DriveCandidate {
            id: id.into(),
            name: format!("{vendor} {gb}GB"),
            vendor: Some(vendor.into()),
            capacity_bytes: gb * GB,
        }
    }

    #[test]
    fn unknown_club_kit_has_three_roles() {
        let r = Ruleset::builtin();
        let preset = r.preset("unknown_club").unwrap();
        let req = KitRequest {
            targets: preset.devices.clone(),
            redundancy: preset.redundancy,
            library_bytes: 94 * GB,
            essential_bytes: Some(20 * GB),
            drives: vec![
                drive("a", "SanDisk", 128),
                drive("b", "Kingston", 32),
                drive("c", "SanDisk", 256),
                drive("d", "Samsung", 128),
            ],
        };
        let plan = plan_kit(&req, &r);
        let roles: Vec<Role> = plan.roles.iter().map(|p| p.role).collect();
        assert_eq!(roles, vec![Role::Main, Role::LegacyRescue, Role::Backup]);
        let main = &plan.roles[0];
        assert_eq!(main.assigned_drive.as_deref(), Some("c"));
        assert_eq!(
            main.library_formats,
            vec![LibraryFormat::RekordboxDeviceLibrary, LibraryFormat::RekordboxOneLibrary]
        );
        assert!(!main.target_models.contains(&"CDJ-2000".to_string()));
        let legacy = &plan.roles[1];
        assert_eq!(legacy.assigned_drive.as_deref(), Some("b"));
        assert_eq!(legacy.library_formats, vec![LibraryFormat::RekordboxDeviceLibrary]);
        assert_eq!(legacy.audio, AudioPolicy::TargetSafeOnly);
        assert!(legacy.target_models.contains(&"CDJ-2000".to_string()));
        // Backup avoids Main's brand when it can.
        let backup = &plan.roles[2];
        assert_eq!(backup.assigned_drive.as_deref(), Some("d"));
        assert!(plan.not_covered.is_empty(), "{:?}", plan.not_covered);
    }

    #[test]
    fn small_backup_holds_the_essentials() {
        let r = Ruleset::builtin();
        let req = KitRequest {
            targets: vec!["cdj-3000".into()],
            redundancy: true,
            library_bytes: 94 * GB,
            essential_bytes: Some(20 * GB),
            drives: vec![drive("a", "SanDisk", 128), drive("b", "Samsung", 64)],
        };
        let plan = plan_kit(&req, &r);
        let backup = plan.roles.iter().find(|p| p.role == Role::Backup).unwrap();
        assert_eq!(backup.assigned_drive.as_deref(), Some("b"));
        assert_eq!(backup.content, ContentPolicy::EssentialPlaylists);
    }

    #[test]
    fn too_small_drives_are_explained() {
        let r = Ruleset::builtin();
        let req = KitRequest {
            targets: vec!["cdj-3000".into()],
            redundancy: false,
            library_bytes: 94 * GB,
            essential_bytes: None,
            drives: vec![drive("a", "Kingston", 32)],
        };
        let plan = plan_kit(&req, &r);
        assert_eq!(plan.roles.len(), 1);
        assert!(plan.roles[0].assigned_drive.is_none());
        assert!(plan.roles[0].drive_notes[0].contains("128 GB"), "{:?}", plan.roles[0].drive_notes);
        assert!(plan.roles[0].drive_notes[1].contains("too small"));
    }

    #[test]
    fn engine_and_mixed_library_formats() {
        let r = Ruleset::builtin();
        let mixed = r.devices_by_id(&r.preset("mixed").unwrap().devices);
        let (formats, uncovered) = required_library_formats(&mixed);
        assert_eq!(
            formats,
            vec![
                LibraryFormat::RekordboxDeviceLibrary,
                LibraryFormat::RekordboxOneLibrary,
                LibraryFormat::EngineDatabase
            ]
        );
        assert!(uncovered.is_empty());
    }

    #[test]
    fn fit_respects_priority_and_reports_exclusions() {
        let sizes: HashMap<u32, u64> = (1..=10).map(|i| (i, 2 * GB)).collect();
        let playlists = vec![
            PlaylistSize { id: 1, name: "Peak".into(), track_ids: vec![1, 2, 3] },
            PlaylistSize { id: 2, name: "Huge".into(), track_ids: vec![4, 5, 6, 7, 8, 9] },
            PlaylistSize { id: 3, name: "Closing".into(), track_ids: vec![3, 10] },
        ];
        // 16 GB drive, budget about 14.2 GB. Peak adds 4 GB (track 2 is incompatible),
        // Huge would add 12 GB and is skipped, Closing adds 2 GB.
        let r = fit_playlists(&playlists, &sizes, 16 * GB, |t| t != 2);
        assert_eq!(r.included_playlists, vec![1, 3]);
        assert_eq!(r.excluded_playlists, vec![2]);
        assert_eq!(r.included_tracks, vec![1, 3, 10]);
        assert_eq!(r.skipped_incompatible, vec![2]);
        assert_eq!(r.bytes, 6 * GB);
    }
}
