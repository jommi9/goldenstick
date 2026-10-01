//! Turns what we found on a drive into per-device, per-layer conclusions.
//!
//! The logic is deliberately conservative: a false "Ready" can ruin a gig, a
//! false "Needs attention" only costs a few minutes.

use super::evaluate::{evaluate_audio, TrackVerdict};
use super::model::{Claim, DeviceKind, DeviceProfile, Evidence, Support};
use super::recommend::{recommend_format, FormatRecommendation};
use super::Ruleset;
use crate::audio::AudioInfo;
use crate::library::{LibraryFormat, LibraryReport};
use crate::media::MediaLayout;
use crate::scan::{AudioScan, TrackProbe};
use boothready_model::{FilesystemKind, PartitionScheme};
use serde::{Deserialize, Serialize};

/// Everything the assessment needs to know about one drive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveFacts {
    pub scheme: PartitionScheme,
    pub filesystem: Option<FilesystemKind>,
    pub fs_dirty: Option<bool>,
    pub primary_is_first: bool,
    pub partition_count: usize,
    pub libraries: Vec<LibraryFormat>,
    /// Formats present but unreadable or empty.
    pub damaged_libraries: Vec<LibraryFormat>,
    pub device_library_missing: u32,
    pub device_library_changed: u32,
    pub engine_missing: u32,
    pub library_warnings: Vec<String>,
    pub tracks: Vec<TrackFacts>,
    pub apple_double: u32,
    pub usb_max_power_ma: Option<u32>,
    /// Catalog ID of the physical USB model, when identified.
    pub usb_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackFacts {
    pub path: String,
    pub info: Option<AudioInfo>,
    /// Set when the file could not be parsed at all.
    pub error: Option<String>,
}

impl DriveFacts {
    pub fn from_parts(
        layout: &MediaLayout,
        libs: &LibraryReport,
        audio: &AudioScan,
        usb_power: Option<u32>,
        usb_model: Option<String>,
    ) -> DriveFacts {
        let primary = layout.primary();
        let fs = primary.and_then(|p| p.filesystem.as_ref());
        let mut damaged = Vec::new();
        let (mut dl_missing, mut dl_changed) = (0, 0);
        if let Some(rb) = &libs.rekordbox {
            if let Some(dl) = &rb.device_library {
                if dl.parse_error.is_some() {
                    damaged.push(LibraryFormat::RekordboxDeviceLibrary);
                }
                dl_missing = dl.missing.len() as u32;
                dl_changed = dl.size_mismatch.len() as u32;
            }
            if rb.one_library.as_ref().is_some_and(|o| o.size_bytes == 0) {
                damaged.push(LibraryFormat::RekordboxOneLibrary);
            }
        }
        let mut engine_missing = 0;
        if let Some(e) = &libs.engine {
            if e.error.is_some() {
                damaged.push(if e.legacy { LibraryFormat::EnginePrimeLegacy } else { LibraryFormat::EngineDatabase });
            }
            engine_missing = e.missing.len() as u32;
        }
        let library_warnings = libs.rekordbox.as_ref().map(|r| r.warnings.clone()).unwrap_or_default();
        DriveFacts {
            scheme: layout.scheme,
            filesystem: fs.map(|f| f.kind),
            fs_dirty: fs.and_then(|f| f.dirty),
            primary_is_first: layout.primary_is_first(),
            partition_count: layout.partitions.len(),
            libraries: libs.formats.clone(),
            damaged_libraries: damaged,
            device_library_missing: dl_missing,
            device_library_changed: dl_changed,
            engine_missing,
            library_warnings,
            tracks: audio
                .tracks
                .iter()
                .map(|t| match &t.probe {
                    TrackProbe::Ok(info) => TrackFacts { path: t.path.clone(), info: Some(info.clone()), error: None },
                    TrackProbe::Unreadable(e) => {
                        TrackFacts { path: t.path.clone(), info: None, error: Some(e.clone()) }
                    }
                })
                .collect(),
            apple_double: audio.apple_double,
            usb_max_power_ma: usb_power,
            usb_model,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Physical,
    Partition,
    Filesystem,
    Library,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerStatus {
    Pass,
    Warn,
    Fail,
    Unknown,
    Info,
}

/// Overall outcome for a device, ordered from best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ready,
    ExpectedToWork,
    Partial,
    Unknown,
    AtRisk,
    FixNeeded,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Ready => "Ready",
            Verdict::ExpectedToWork => "Expected to work",
            Verdict::Partial => "Partly ready",
            Verdict::Unknown => "Not enough data",
            Verdict::AtRisk => "At risk",
            Verdict::FixNeeded => "Fix needed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerResult {
    pub layer: Layer,
    pub status: LayerStatus,
    pub evidence: Evidence,
    /// What this layer contributes to the device verdict.
    pub impact: Verdict,
    pub headline: String,
    pub detail: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixKind {
    Reformat { scheme: PartitionScheme, filesystem: FilesystemKind },
    ExportRekordbox { formats: Vec<LibraryFormat> },
    ReexportRekordbox,
    ExportEngine,
    RepairFilesystem,
    ReviewTracks { count: u32 },
    RemoveAppleDouble { count: u32 },
    UseAnotherUsb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    #[serde(flatten)]
    pub kind: FixKind,
    pub title: String,
    pub detail: String,
    /// Erases the drive.
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackIssue {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTally {
    pub total: u32,
    pub supported: u32,
    pub unsupported: u32,
    pub unknown: u32,
    pub unreadable: u32,
    /// Up to 200 affected tracks, for the detail view.
    pub issues: Vec<TrackIssue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceAssessment {
    pub device_id: String,
    pub manufacturer: String,
    pub model: String,
    pub legacy: bool,
    pub verdict: Verdict,
    pub summary: String,
    pub layers: Vec<LayerResult>,
    pub audio: AudioTally,
    pub notes: Vec<String>,
    pub fixes: Vec<Fix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub label: String,
    pub status: LayerStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackSummary {
    pub scanned: u32,
    /// Plays on every selected device.
    pub compatible_everywhere: u32,
    pub need_attention: u32,
    pub unreadable: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveAssessment {
    pub overall: Verdict,
    pub headline: String,
    pub checks: Vec<Check>,
    pub tracks: TrackSummary,
    pub devices: Vec<DeviceAssessment>,
    pub fixes: Vec<Fix>,
    pub recommended_format: FormatRecommendation,
}

const MAX_ISSUES: usize = 200;

fn tracks(n: u32) -> String {
    if n == 1 {
        "1 track".into()
    } else {
        format!("{n} tracks")
    }
}

fn pass_impact(ev: Evidence) -> Verdict {
    if ev.is_strong() {
        Verdict::Ready
    } else {
        Verdict::ExpectedToWork
    }
}

fn layer(
    layer: Layer,
    status: LayerStatus,
    evidence: Evidence,
    impact: Verdict,
    headline: impl Into<String>,
    detail: impl Into<String>,
    source: Option<&str>,
) -> LayerResult {
    LayerResult {
        layer,
        status,
        evidence,
        impact,
        headline: headline.into(),
        detail: detail.into(),
        source: source.map(str::to_string),
    }
}

fn evidence_suffix(ev: Evidence) -> &'static str {
    match ev {
        Evidence::Vendor => " (manufacturer documentation)",
        Evidence::Lab => " (BoothReady lab test)",
        Evidence::Community => " (community reports)",
        Evidence::Inferred => " (inferred from related models, not documented)",
        Evidence::Unknown => "",
    }
}

fn physical_layer(d: &DeviceProfile, facts: &DriveFacts, rules: &Ruleset) -> LayerResult {
    if let Some(model) = &facts.usb_model {
        let obs = rules.observations.iter().find(|o| {
            &o.usb_model == model
                && o.device == d.id
                && Some(o.filesystem) == facts.filesystem
                && o.scheme == facts.scheme
        });
        if let Some(o) = obs {
            return if o.worked {
                layer(
                    Layer::Physical,
                    LayerStatus::Pass,
                    o.evidence,
                    Verdict::Ready,
                    "Tested on this player",
                    format!("This USB model was tested on a {} on {}.", d.model, o.tested_on),
                    None,
                )
            } else {
                layer(
                    Layer::Physical,
                    LayerStatus::Fail,
                    o.evidence,
                    Verdict::FixNeeded,
                    "Known not to work",
                    format!(
                        "This USB model failed on a {} in testing. {}",
                        d.model,
                        o.notes.clone().unwrap_or_default()
                    ),
                    None,
                )
            };
        }
    }
    if let (Some(draw), Some(limit)) = (facts.usb_max_power_ma, d.usb_power_ma) {
        if draw > limit.value {
            return layer(
                Layer::Physical,
                LayerStatus::Warn,
                limit.evidence,
                Verdict::AtRisk,
                "Draws more power than the player supplies",
                format!(
                    "This drive asks for {draw} mA; the {} supplies {} mA{}. It may not be detected.",
                    d.model,
                    limit.value,
                    evidence_suffix(limit.evidence)
                ),
                d.source("usb_power_ma"),
            );
        }
    }
    layer(
        Layer::Physical,
        LayerStatus::Unknown,
        Evidence::Unknown,
        Verdict::Ready,
        "This exact USB model is untested on this player",
        format!(
            "The format is what matters most, but this USB model hasn't been physically tested on a {} yet.",
            d.model
        ),
        None,
    )
}

fn claim_layer(l: Layer, claim: Claim, thing: &str, d: &DeviceProfile, source: Option<&str>) -> LayerResult {
    let ev = claim.evidence;
    match claim.support {
        Support::Supported => layer(
            l,
            LayerStatus::Pass,
            ev,
            pass_impact(ev),
            format!("{thing} is supported"),
            format!("The {} reads {thing}{}.", d.model, evidence_suffix(ev)),
            source,
        ),
        Support::Unsupported => layer(
            l,
            LayerStatus::Fail,
            ev,
            Verdict::FixNeeded,
            format!("{thing} is not supported"),
            format!("The {} can't read {thing}{}.", d.model, evidence_suffix(ev)),
            source,
        ),
        Support::Unreliable => layer(
            l,
            LayerStatus::Warn,
            ev,
            Verdict::AtRisk,
            format!("{thing} may not be recognised"),
            format!("{thing} may not be recognised by the {}{}.", d.model, evidence_suffix(ev)),
            source,
        ),
        Support::Unknown => layer(
            l,
            LayerStatus::Unknown,
            Evidence::Unknown,
            Verdict::Unknown,
            format!("No data on {thing}"),
            format!("BoothReady has no reliable information on whether the {} reads {thing}.", d.model),
            source,
        ),
    }
}

fn library_layer(d: &DeviceProfile, facts: &DriveFacts) -> (LayerResult, Vec<Fix>) {
    let src = d.source("library_formats");
    let usable: Vec<LibraryFormat> =
        facts.libraries.iter().copied().filter(|f| !facts.damaged_libraries.contains(f)).collect();
    let readable_present: Vec<(LibraryFormat, Claim)> =
        usable.iter().map(|f| (*f, d.library(*f))).filter(|(_, c)| c.support == Support::Supported).collect();
    let wanted: Vec<LibraryFormat> = d.readable_libraries().into_iter().map(|(f, _)| f).collect();
    let export_fix = |formats: Vec<LibraryFormat>| -> Fix {
        if formats.iter().any(|f| matches!(f, LibraryFormat::EngineDatabase)) {
            Fix {
                kind: FixKind::ExportEngine,
                title: "Prepare this USB in Engine DJ".into(),
                detail: "Open Engine DJ, add the playlists you need to this drive and let it finish syncing. BoothReady re-checks the drive when you're done.".into(),
                destructive: false,
            }
        } else {
            let mut all = formats.clone();
            // Current rekordbox writes both formats in one export; ask for both
            // so the drive also keeps working on older players.
            for f in [LibraryFormat::RekordboxDeviceLibrary, LibraryFormat::RekordboxOneLibrary] {
                if !all.contains(&f) {
                    all.push(f);
                }
            }
            Fix {
                kind: FixKind::ExportRekordbox { formats: all },
                title: "Export this USB again from rekordbox".into(),
                detail: "Use rekordbox 7.2.11 or later and export to this drive with both Device Library and OneLibrary. BoothReady picks up the new export automatically.".into(),
                destructive: false,
            }
        }
    };

    if let Some((fmt, claim)) = readable_present.iter().max_by_key(|(_, c)| c.evidence).copied() {
        let mut ev = claim.evidence;
        let mut notes = Vec::new();
        if fmt == LibraryFormat::RekordboxOneLibrary {
            // Presence only: the database is encrypted and we can't look inside.
            ev = ev.min(Evidence::Inferred);
            notes.push(
                "BoothReady can confirm the OneLibrary database is present but can't read inside it yet.".to_string(),
            );
        }
        let missing = match fmt {
            LibraryFormat::RekordboxDeviceLibrary => facts.device_library_missing,
            LibraryFormat::EngineDatabase => facts.engine_missing,
            _ => 0,
        };
        let changed = if fmt == LibraryFormat::RekordboxDeviceLibrary { facts.device_library_changed } else { 0 };
        let stale = fmt.label().starts_with("rekordbox") && !facts.library_warnings.is_empty();
        if missing > 0 || changed > 0 || stale {
            let mut detail = Vec::new();
            if missing > 0 {
                detail.push(format!(
                    "{missing} tracks in the {} are missing from the drive and won't load.",
                    fmt.label()
                ));
            }
            if changed > 0 {
                detail.push(format!(
                    "{changed} tracks changed after the export, so cue points and beatgrids may be off."
                ));
            }
            detail.extend(facts.library_warnings.iter().cloned());
            let fix = if fmt == LibraryFormat::EngineDatabase {
                export_fix(vec![LibraryFormat::EngineDatabase])
            } else {
                Fix {
                    kind: FixKind::ReexportRekordbox,
                    title: "Export this USB again from rekordbox".into(),
                    detail: "The export on this drive no longer matches its files. A fresh export from rekordbox fixes that.".into(),
                    destructive: false,
                }
            };
            return (
                layer(
                    Layer::Library,
                    LayerStatus::Warn,
                    ev,
                    Verdict::Partial,
                    format!("{} is out of date", fmt.label()),
                    detail.join(" "),
                    src,
                ),
                vec![fix],
            );
        }
        let mut detail =
            format!("The {} reads the {} on this drive{}.", d.model, fmt.label(), evidence_suffix(claim.evidence));
        for n in notes {
            detail.push(' ');
            detail.push_str(&n);
        }
        return (
            layer(
                Layer::Library,
                LayerStatus::Pass,
                ev,
                pass_impact(ev),
                format!("{} found", fmt.label()),
                detail,
                src,
            ),
            vec![],
        );
    }

    let unknown_present: Vec<LibraryFormat> =
        usable.iter().copied().filter(|f| d.library(*f).support == Support::Unknown).collect();
    let damaged: Vec<LibraryFormat> = facts.damaged_libraries.iter().copied().filter(|f| wanted.contains(f)).collect();
    if !damaged.is_empty() {
        let name = damaged[0].label();
        return (
            layer(
                Layer::Library,
                LayerStatus::Fail,
                Evidence::Vendor,
                Verdict::FixNeeded,
                format!("{name} is damaged"),
                format!("The {name} on this drive can't be read. The {} won't show your playlists.", d.model),
                src,
            ),
            vec![export_fix(damaged)],
        );
    }
    if usable.is_empty() {
        // No DJ library at all: the player can still browse folders.
        return match d.folder_browsing.support {
            Support::Supported => (
                layer(
                    Layer::Library,
                    LayerStatus::Warn,
                    d.folder_browsing.evidence,
                    Verdict::Partial,
                    "Folder browsing only",
                    format!("There's no DJ library on this drive. The {} can browse folders, but you won't have playlists, cue points or beatgrids.", d.model),
                    src,
                ),
                vec![export_fix(wanted)],
            ),
            _ => (
                layer(Layer::Library, LayerStatus::Fail, d.folder_browsing.evidence, Verdict::FixNeeded, "No library this player can use", format!("The {} needs a prepared library on the drive.", d.model), src),
                vec![export_fix(wanted)],
            ),
        };
    }
    if !unknown_present.is_empty() && wanted.is_empty() {
        return (
            layer(
                Layer::Library,
                LayerStatus::Unknown,
                Evidence::Unknown,
                Verdict::Unknown,
                "No data on library support",
                format!("BoothReady has no reliable information on which library the {} reads.", d.model),
                src,
            ),
            vec![],
        );
    }
    let need: Vec<&str> = wanted.iter().map(|f| f.label()).collect();
    let have: Vec<&str> = usable.iter().map(|f| f.label()).collect();
    let mut detail = format!("The {} reads {}. This USB only has {}.", d.model, need.join(" or "), have.join(" and "));
    if !unknown_present.is_empty() {
        detail.push_str(&format!(
            " Whether it can also read the {} isn't documented, so BoothReady doesn't count on it.",
            unknown_present.iter().map(|f| f.label()).collect::<Vec<_>>().join(" or ")
        ));
    }
    let short = wanted.first().map(|f| f.label()).unwrap_or("A supported library");
    (
        layer(
            Layer::Library,
            LayerStatus::Fail,
            d.library(wanted.first().copied().unwrap_or(usable[0])).evidence,
            Verdict::FixNeeded,
            format!("{short} missing"),
            detail,
            src,
        ),
        vec![export_fix(wanted)],
    )
}

fn audio_layer(d: &DeviceProfile, facts: &DriveFacts) -> (LayerResult, AudioTally) {
    let mut t = AudioTally::default();
    let mut weakest = Evidence::Lab;
    for tr in &facts.tracks {
        t.total += 1;
        match (&tr.info, &tr.error) {
            (Some(info), _) => match evaluate_audio(d, info) {
                TrackVerdict::Supported { evidence } => {
                    t.supported += 1;
                    weakest = weakest.min(evidence);
                }
                TrackVerdict::Unsupported { reason, .. } => {
                    t.unsupported += 1;
                    if t.issues.len() < MAX_ISSUES {
                        t.issues.push(TrackIssue { path: tr.path.clone(), reason });
                    }
                }
                TrackVerdict::Unknown { reason } => {
                    t.unknown += 1;
                    if t.issues.len() < MAX_ISSUES {
                        t.issues.push(TrackIssue { path: tr.path.clone(), reason });
                    }
                }
            },
            (None, err) => {
                t.unreadable += 1;
                if t.issues.len() < MAX_ISSUES {
                    t.issues.push(TrackIssue {
                        path: tr.path.clone(),
                        reason: err.clone().unwrap_or_else(|| "unreadable".into()),
                    });
                }
            }
        }
    }
    let src = d.source("audio");
    let result = if t.total == 0 {
        layer(
            Layer::Audio,
            LayerStatus::Warn,
            Evidence::Unknown,
            Verdict::Partial,
            "No music found",
            "There are no audio files on this drive.",
            src,
        )
    } else if t.supported == 0 {
        layer(
            Layer::Audio,
            LayerStatus::Fail,
            Evidence::Vendor,
            Verdict::FixNeeded,
            "None of the tracks will play",
            format!("None of the {} tracks on this drive are in a format the {} plays.", t.total, d.model),
            src,
        )
    } else if t.unsupported + t.unreadable > 0 {
        let mut parts = Vec::new();
        if t.unsupported > 0 {
            parts.push(format!("{} won't play on the {}", tracks(t.unsupported), d.model));
        }
        if t.unreadable > 0 {
            parts.push(format!(
                "{} {} damaged or unreadable",
                tracks(t.unreadable),
                if t.unreadable == 1 { "is" } else { "are" }
            ));
        }
        if t.unknown > 0 {
            parts.push(format!("{} couldn't be checked", tracks(t.unknown)));
        }
        layer(
            Layer::Audio,
            LayerStatus::Warn,
            weakest,
            Verdict::Partial,
            format!("{} of {} tracks need attention", t.total - t.supported, t.total),
            format!("{}.", parts.join(", ")),
            src,
        )
    } else if t.unknown > 0 {
        layer(
            Layer::Audio,
            LayerStatus::Unknown,
            Evidence::Unknown,
            Verdict::Partial,
            format!("{} couldn't be checked", tracks(t.unknown)),
            format!("BoothReady has no compatibility data for {} of the tracks on the {}.", t.unknown, d.model),
            src,
        )
    } else {
        layer(
            Layer::Audio,
            LayerStatus::Pass,
            weakest,
            pass_impact(weakest),
            format!("All {} tracks will play", t.total),
            format!("Every track is in a format the {} plays{}.", d.model, evidence_suffix(weakest)),
            src,
        )
    };
    (result, t)
}

pub fn assess_device(
    d: &DeviceProfile,
    facts: &DriveFacts,
    rules: &Ruleset,
    format: &FormatRecommendation,
) -> DeviceAssessment {
    let mut fixes = Vec::new();
    let mut notes: Vec<String> =
        d.quirks.iter().filter(|q| q.applies(facts.scheme, facts.filesystem)).map(|q| q.text.clone()).collect();
    let physical = physical_layer(d, facts, rules);

    let mut partition =
        claim_layer(Layer::Partition, d.partition(facts.scheme), facts.scheme.label(), d, d.source("partition_tables"));
    if facts.scheme == PartitionScheme::Mbr && !facts.primary_is_first && facts.partition_count > 1 {
        partition.status = LayerStatus::Warn;
        partition.impact = partition.impact.max(Verdict::AtRisk);
        partition.detail.push_str(" The music partition isn't the first one, and players only mount the first.");
    }
    let mut filesystem = match facts.filesystem {
        Some(fs) => claim_layer(Layer::Filesystem, d.filesystem(fs), fs.label(), d, d.source("filesystems")),
        None => layer(
            Layer::Filesystem,
            LayerStatus::Fail,
            Evidence::Unknown,
            Verdict::FixNeeded,
            "No readable filesystem",
            "BoothReady couldn't find a filesystem on this drive.",
            None,
        ),
    };
    if facts.fs_dirty == Some(true) && filesystem.status == LayerStatus::Pass {
        filesystem.status = LayerStatus::Warn;
        filesystem.impact = filesystem.impact.max(Verdict::AtRisk);
        filesystem.headline = "Not ejected safely last time".into();
        filesystem.detail.push_str(" The drive was removed without ejecting, so the filesystem may have errors.");
        fixes.push(Fix {
            kind: FixKind::RepairFilesystem,
            title: "Check and repair the drive".into(),
            detail: "Runs the operating system's filesystem check. Nothing is erased.".into(),
            destructive: false,
        });
    }
    let layout_bad = matches!(partition.status, LayerStatus::Fail | LayerStatus::Warn | LayerStatus::Unknown)
        || matches!(filesystem.status, LayerStatus::Fail | LayerStatus::Unknown);
    if layout_bad && (format.scheme, Some(format.filesystem)) != (facts.scheme, facts.filesystem) {
        fixes.push(Fix {
            kind: FixKind::Reformat { scheme: format.scheme, filesystem: format.filesystem },
            title: format!("Rebuild as {} + {}", format.scheme.label(), format.filesystem.label()),
            detail: format.why.clone(),
            destructive: true,
        });
    }
    if physical.status == LayerStatus::Warn || physical.status == LayerStatus::Fail {
        fixes.push(Fix {
            kind: FixKind::UseAnotherUsb,
            title: "Use a different USB for this player".into(),
            detail: physical.detail.clone(),
            destructive: false,
        });
    }
    let (library, lib_fixes) = library_layer(d, facts);
    fixes.extend(lib_fixes);
    let (audio, tally) = audio_layer(d, facts);
    if tally.unsupported + tally.unreadable > 0 && tally.supported > 0 {
        fixes.push(Fix {
            kind: FixKind::ReviewTracks { count: tally.unsupported + tally.unreadable },
            title: format!("Review {} tracks", tally.unsupported + tally.unreadable),
            detail: "See which tracks won't play and why, then exclude them or replace them with compatible copies."
                .into(),
            destructive: false,
        });
    }
    if facts.apple_double > 0 && d.folder_browsing.support == Support::Supported {
        notes.push(format!(
            "{} hidden macOS '._' files sit next to your music. They show up as unplayable tracks when browsing folders.",
            facts.apple_double
        ));
        fixes.push(Fix {
            kind: FixKind::RemoveAppleDouble { count: facts.apple_double },
            title: "Remove macOS '._' files".into(),
            detail:
                "Deletes only the hidden metadata files macOS leaves on FAT and exFAT drives. Your music is untouched."
                    .into(),
            destructive: false,
        });
    }
    let layers = vec![physical, partition, filesystem, library, audio];
    let verdict = layers.iter().map(|l| l.impact).max().unwrap_or(Verdict::Unknown);
    let worst = layers
        .iter()
        .filter(|l| l.impact == verdict && l.status != LayerStatus::Pass)
        .map(|l| l.headline.clone())
        .next();
    let summary = match (verdict, worst) {
        (Verdict::Ready, _) => "Ready".to_string(),
        (Verdict::ExpectedToWork, _) => "Expected to work, not verified".to_string(),
        (v, Some(h)) => format!("{}: {}", v.label(), h),
        (v, None) => v.label().to_string(),
    };
    DeviceAssessment {
        device_id: d.id.clone(),
        manufacturer: d.manufacturer.clone(),
        model: d.model.clone(),
        legacy: d.legacy,
        verdict,
        summary,
        layers,
        audio: tally,
        notes,
        fixes,
    }
}

/// Assess one drive against a set of target devices.
pub fn assess_drive(facts: &DriveFacts, targets: &[&DeviceProfile], rules: &Ruleset) -> DriveAssessment {
    let format = recommend_format(targets);
    let devices: Vec<DeviceAssessment> = targets.iter().map(|d| assess_device(d, facts, rules, &format)).collect();
    let overall = devices.iter().map(|d| d.verdict).max().unwrap_or(Verdict::Unknown);

    let mut fixes: Vec<Fix> = Vec::new();
    for f in devices.iter().flat_map(|d| d.fixes.iter()) {
        let dup = fixes.iter_mut().find(|e| std::mem::discriminant(&e.kind) == std::mem::discriminant(&f.kind));
        match dup {
            Some(existing) => {
                if let (FixKind::ExportRekordbox { formats: a }, FixKind::ExportRekordbox { formats: b }) =
                    (&mut existing.kind, &f.kind)
                {
                    for x in b {
                        if !a.contains(x) {
                            a.push(*x);
                        }
                    }
                }
                if let (FixKind::ReviewTracks { count: a }, FixKind::ReviewTracks { count: b }) =
                    (&mut existing.kind, &f.kind)
                {
                    *a = (*a).max(*b);
                }
            }
            None => fixes.push(f.clone()),
        }
    }
    // Non-destructive fixes first; erasing is the last resort.
    fixes.sort_by_key(|f| f.destructive);

    let mut tracks = TrackSummary { scanned: facts.tracks.len() as u32, ..Default::default() };
    for tr in &facts.tracks {
        match &tr.info {
            None => tracks.unreadable += 1,
            Some(info) => {
                if !targets.is_empty() && targets.iter().all(|d| evaluate_audio(d, info).is_supported()) {
                    tracks.compatible_everywhere += 1;
                }
            }
        }
    }
    tracks.need_attention = tracks.scanned - tracks.compatible_everywhere;

    let checks = drive_checks(facts, targets);
    let headline = headline(overall, &devices, facts);
    DriveAssessment { overall, headline, checks, tracks, devices, fixes, recommended_format: format }
}

fn drive_checks(facts: &DriveFacts, targets: &[&DeviceProfile]) -> Vec<Check> {
    let mut checks = Vec::new();
    let fs_ok = |fs: FilesystemKind| targets.iter().all(|d| d.filesystem(fs).support == Support::Supported);
    match facts.filesystem {
        Some(fs) => checks.push(Check {
            label: fs.label().into(),
            status: if fs_ok(fs) { LayerStatus::Pass } else { LayerStatus::Fail },
        }),
        None => checks.push(Check { label: "No filesystem".into(), status: LayerStatus::Fail }),
    }
    let scheme_ok = targets.iter().all(|d| d.partition(facts.scheme).support == Support::Supported);
    checks.push(Check {
        label: facts.scheme.label().into(),
        status: if scheme_ok { LayerStatus::Pass } else { LayerStatus::Fail },
    });
    for fmt in
        [LibraryFormat::RekordboxDeviceLibrary, LibraryFormat::RekordboxOneLibrary, LibraryFormat::EngineDatabase]
    {
        let present = facts.libraries.contains(&fmt) && !facts.damaged_libraries.contains(&fmt);
        // Needed when some target reads only this format among what it supports.
        let needed = targets.iter().any(|d| {
            let readable = d.readable_libraries();
            readable.iter().any(|(f, _)| *f == fmt)
                && !readable
                    .iter()
                    .any(|(f, _)| *f != fmt && facts.libraries.contains(f) && !facts.damaged_libraries.contains(f))
        });
        let status = if present {
            LayerStatus::Pass
        } else if needed {
            LayerStatus::Fail
        } else {
            LayerStatus::Info
        };
        let label = if present || needed { fmt.label().to_string() } else { format!("{} not detected", fmt.label()) };
        checks.push(Check { label, status });
    }
    checks
}

fn headline(overall: Verdict, devices: &[DeviceAssessment], facts: &DriveFacts) -> String {
    let ready = devices.iter().filter(|d| d.verdict <= Verdict::ExpectedToWork).count();
    match overall {
        Verdict::Ready => "Ready for all selected equipment".into(),
        Verdict::ExpectedToWork => "Expected to work on all selected equipment".into(),
        _ if facts.tracks.is_empty() && facts.libraries.is_empty() => "Empty drive, ready to prepare".into(),
        Verdict::Partial => {
            format!("Works on {ready} of {} players, with some tracks needing attention", devices.len())
        }
        _ if ready > 0 => format!("Ready for {ready} of {} players", devices.len()),
        _ => "Needs attention before the gig".into(),
    }
}

/// Coarse first impression shown before the user picks a target (PRD §15).
pub fn general_headline(facts: &DriveFacts, rules: &Ruleset) -> String {
    let (Some(fs), scheme) = (facts.filesystem, facts.scheme) else {
        return "Not usable in DJ gear as formatted".into();
    };
    let strict = |d: &DeviceProfile| {
        d.partition(scheme).support == Support::Supported && d.filesystem(fs).support == Support::Supported
    };
    let loose = |d: &DeviceProfile| {
        matches!(d.partition(scheme).support, Support::Supported | Support::Unreliable)
            && matches!(d.filesystem(fs).support, Support::Supported | Support::Unreliable)
    };
    let pioneer: Vec<&DeviceProfile> = rules.devices.iter().filter(|d| d.family == "alphatheta").collect();
    let legacy_ok = pioneer.iter().filter(|d| d.legacy).all(|d| strict(d));
    let current_ok = pioneer.iter().filter(|d| !d.legacy).any(|d| strict(d));
    if legacy_ok && current_ok {
        "Good general-purpose DJ USB".into()
    } else if current_ok {
        "Fine for newer players, won't work on older ones".into()
    } else if pioneer.iter().any(|d| loose(d)) {
        "DJ players may not recognise this drive as formatted".into()
    } else if rules.devices.iter().any(|d| d.kind != DeviceKind::Software && strict(d)) {
        "Only suits some DJ equipment as formatted".into()
    } else {
        "DJ players won't recognise this drive as formatted".into()
    }
}
