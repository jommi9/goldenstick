//! Repeatable virtual scenarios used by the CLI and the desktop test-lab UI.
//!
//! Every write in this module is scoped to a fixture directory. The module
//! never opens a native disk and never prepares, copies or ejects a real USB.

#![cfg_attr(test, allow(clippy::unwrap_used))]

use anyhow::{bail, Context, Result};
use boothready_core::copy::{plan_copy, run_copy, CopyTarget};
use boothready_core::demo::{create_stick, DemoStick, STICKS};
use boothready_core::drive::{analyze_drive, DriveReport};
use boothready_core::format::build_fat32;
use boothready_core::identify::UsbCatalog;
use boothready_core::library::LibraryFormat;
use boothready_core::media::inspect;
use boothready_core::privileged::{DeviceFingerprint, HelperEvent, HelperRequest};
use boothready_core::rules::{assess_drive, FixKind, Ruleset, Verdict};
use boothready_core::verify::{verify_volume, Expectations, VerifyRequest};
use boothready_helper::{handle, Backend, DemoBackend};
use boothready_model::{FilesystemKind, PartitionScheme};
use boothready_platform::Platform;
use serde::Serialize;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
enum ScenarioKind {
    Acceptance,
    Empty,
    DualLibrary,
    PrepareFat32,
    CopyAndVerify,
    MountedDirtyFlag,
}

#[derive(Clone, Copy)]
struct Scenario {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    kind: ScenarioKind,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        id: "acceptance-main",
        title: "PRD acceptance drive",
        description: "GPT + exFAT with a Device Library and intentionally problematic tracks",
        kind: ScenarioKind::Acceptance,
    },
    Scenario {
        id: "empty-drive",
        title: "Empty new drive",
        description: "A blank GPT + exFAT stick with no DJ library",
        kind: ScenarioKind::Empty,
    },
    Scenario {
        id: "dual-library-export",
        title: "Complete rekordbox export",
        description: "Device Library and OneLibrary with clean audio",
        kind: ScenarioKind::DualLibrary,
    },
    Scenario {
        id: "prepare-fat32",
        title: "Safe fixture preparation",
        description: "Run the real helper request against an image-backed demo stick",
        kind: ScenarioKind::PrepareFat32,
    },
    Scenario {
        id: "copy-and-verify",
        title: "Backup copy and read-back",
        description: "Copy a complete export to another fixture and verify every file",
        kind: ScenarioKind::CopyAndVerify,
    },
    Scenario {
        id: "mounted-dirty-flag",
        title: "Dirty flag while mounted",
        description: "A FAT32 image whose clean-shutdown bit is cleared, the way macOS leaves it while mounted, checked mounted and unmounted",
        kind: ScenarioKind::MountedDirtyFlag,
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct ScenarioInfo {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VirtualResult {
    pub id: String,
    pub title: String,
    pub description: String,
    pub passed: bool,
    pub duration_ms: u128,
    pub checks: Vec<Check>,
    pub artifact: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VirtualSuite {
    pub kind: &'static str,
    pub passed: bool,
    pub root: String,
    pub scenarios: Vec<VirtualResult>,
}

/// The prepare scenario formats a 31 GB image. NTFS allocates every byte of
/// it because the file is not sparse there, so the run fails on a machine
/// with less free space than that. The scenario is left out on Windows, like
/// the CLI acceptance test.
fn runs_here(scenario: &Scenario) -> bool {
    !(cfg!(windows) && matches!(scenario.kind, ScenarioKind::PrepareFat32))
}

/// The scenarios that can run on this operating system.
pub fn scenarios() -> Vec<ScenarioInfo> {
    SCENARIOS
        .iter()
        .filter(|s| runs_here(s))
        .map(|s| ScenarioInfo { id: s.id.into(), title: s.title.into(), description: s.description.into() })
        .collect()
}

/// Run one scenario or the complete matrix. `root` is the report directory;
/// it must not exist yet. If omitted, a timestamped `test-reports/virtual-*` directory is used.
pub fn run_virtual_suite(
    selected: Option<&str>,
    keep_fixtures: bool,
    root: Option<PathBuf>,
    rules: &Ruleset,
    catalog: &UsbCatalog,
) -> Result<VirtualSuite> {
    let selected: Vec<&Scenario> = match selected {
        None | Some("all") => SCENARIOS.iter().filter(|s| runs_here(s)).collect(),
        Some(id) => {
            let scenario =
                SCENARIOS.iter().find(|s| s.id == id).with_context(|| format!("unknown virtual scenario '{id}'"))?;
            if !runs_here(scenario) {
                bail!("virtual scenario '{id}' formats a 31 GB image, which isn't sparse on NTFS, so it doesn't run on Windows");
            }
            vec![scenario]
        }
    };
    let root = match root {
        Some(root) => {
            if let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent)?;
            }
            fs::create_dir(&root).context("test output directory must not already exist")?;
            root
        }
        None => create_report_directory(Path::new("test-reports"), "virtual")?,
    };

    let mut results = Vec::new();
    for scenario in selected {
        let scenario_root = root.join(scenario.id);
        fs::create_dir_all(&scenario_root)?;
        let mut result = execute_virtual(scenario, &scenario_root, rules, catalog);
        let artifact = scenario_root.join("result.json");
        result.artifact = Some(artifact.display().to_string());
        fs::write(&artifact, serde_json::to_vec_pretty(&result)?)?;
        if !keep_fixtures {
            let _ = fs::remove_dir_all(scenario_root.join("usb"));
        }
        results.push(result);
    }
    Ok(VirtualSuite {
        kind: "virtual",
        passed: results.iter().all(|r| r.passed),
        root: root.display().to_string(),
        scenarios: results,
    })
}

fn execute_virtual(scenario: &Scenario, root: &Path, rules: &Ruleset, catalog: &UsbCatalog) -> VirtualResult {
    let started = Instant::now();
    let mut checks = Vec::new();
    let error = match match scenario.kind {
        ScenarioKind::Acceptance => {
            run_analysis_scenario(root, "sandisk-128", rules, catalog, &mut checks, ScenarioKind::Acceptance)
        }
        ScenarioKind::Empty => {
            run_analysis_scenario(root, "kingston-32", rules, catalog, &mut checks, ScenarioKind::Empty)
        }
        ScenarioKind::DualLibrary => {
            run_analysis_scenario(root, "samsung-64", rules, catalog, &mut checks, ScenarioKind::DualLibrary)
        }
        ScenarioKind::PrepareFat32 => run_prepare_scenario(root, &mut checks),
        ScenarioKind::CopyAndVerify => run_copy_scenario(root, &mut checks),
        ScenarioKind::MountedDirtyFlag => run_mounted_dirty_scenario(root, rules, catalog, &mut checks),
    } {
        Ok(()) => None,
        Err(e) => Some(e.to_string()),
    };
    VirtualResult {
        id: scenario.id.into(),
        title: scenario.title.into(),
        description: scenario.description.into(),
        passed: error.is_none() && checks.iter().all(|c| c.passed),
        duration_ms: started.elapsed().as_millis(),
        checks,
        artifact: None,
        error,
    }
}

fn run_analysis_scenario(
    root: &Path,
    stick_name: &str,
    rules: &Ruleset,
    catalog: &UsbCatalog,
    checks: &mut Vec<Check>,
    kind: ScenarioKind,
) -> Result<()> {
    let stick = stick(stick_name)?;
    let usb = root.join("usb");
    create_stick(&usb, stick)?;
    let platform = boothready_platform::demo::DemoPlatform::new(usb);
    let dev = platform
        .list_devices()?
        .into_iter()
        .find(|d| d.id == format!("demo:{stick_name}"))
        .with_context(|| format!("fixture '{stick_name}' was not created"))?;
    let raw = fs::File::open(&dev.os_path).ok().and_then(|f| inspect(f).ok());
    let report = analyze_drive(&dev, raw, rules, catalog, |_, _| {});
    let targets = rules.preset("unknown_club").context("unknown_club preset is missing")?;
    let devices = rules.devices_by_id(&targets.devices);
    let assessment = assess_drive(&report.facts, &devices, rules);

    match kind {
        ScenarioKind::Acceptance => {
            check(
                checks,
                "partition",
                report.layout.scheme == PartitionScheme::Gpt,
                report.layout.scheme.label().into(),
            );
            check(
                checks,
                "filesystem",
                report.layout.primary_filesystem() == Some(FilesystemKind::Exfat),
                report.layout.primary_filesystem().map(|f| f.label()).unwrap_or("unknown").into(),
            );
            check(
                checks,
                "device-library-only",
                report.libraries.formats == vec![LibraryFormat::RekordboxDeviceLibrary],
                format!("{:?}", report.libraries.formats),
            );
            check(
                checks,
                "audio-scan",
                report.audio.tracks.len() == 13 && unreadable_count(&report) == 1,
                format!("{} tracks, {} unreadable", report.audio.tracks.len(), unreadable_count(&report)),
            );
            check(
                checks,
                "macos-litter",
                report.audio.apple_double == 2,
                format!("{} ._ files", report.audio.apple_double),
            );
            check(checks, "rules-verdict", assessment.overall == Verdict::FixNeeded, assessment.overall.label().into());
        }
        ScenarioKind::Empty => {
            check(
                checks,
                "empty-content",
                report.audio.tracks.is_empty(),
                format!("{} tracks", report.audio.tracks.len()),
            );
            check(checks, "no-library", report.libraries.formats.is_empty(), format!("{:?}", report.libraries.formats));
        }
        ScenarioKind::DualLibrary => {
            let both = report.libraries.formats.contains(&LibraryFormat::RekordboxDeviceLibrary)
                && report.libraries.formats.contains(&LibraryFormat::RekordboxOneLibrary);
            check(checks, "both-libraries", both, format!("{:?}", report.libraries.formats));
            check(
                checks,
                "clean-audio",
                unreadable_count(&report) == 0,
                format!("{} tracks", report.audio.tracks.len()),
            );
            check(
                checks,
                "no-macos-litter",
                report.audio.apple_double == 0,
                format!("{} ._ files", report.audio.apple_double),
            );
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn run_prepare_scenario(root: &Path, checks: &mut Vec<Check>) -> Result<()> {
    let stick = stick("kingston-32")?;
    let usb = root.join("usb");
    create_stick(&usb, stick)?;
    let backend = DemoBackend::new(usb);
    let dev = backend
        .platform()
        .list_devices()?
        .into_iter()
        .find(|d| d.id == "demo:kingston-32")
        .context("prepare fixture was not created")?;
    let fp = DeviceFingerprint::of(&dev);
    let req = HelperRequest::Prepare {
        confirmation: fp.token(),
        expected: fp,
        scheme: PartitionScheme::Mbr,
        filesystem: FilesystemKind::Fat32,
        label: "BR_TEST".into(),
    };
    let mut helper_error = None;
    handle(&backend, req, &mut |event| {
        if let HelperEvent::Error { message, .. } = event {
            helper_error = Some(message);
        }
    });
    if let Some(e) = helper_error {
        bail!("helper failed: {e}");
    }
    let after = backend
        .platform()
        .list_devices()?
        .into_iter()
        .find(|d| d.id == "demo:kingston-32")
        .context("prepared fixture disappeared")?;
    let layout = backend.open_read(&after).and_then(inspect).context("could not inspect prepared image")?;
    check(checks, "helper-request", true, "completed without a real device".into());
    check(checks, "mbr-layout", layout.scheme == PartitionScheme::Mbr, layout.scheme.label().into());
    check(
        checks,
        "fat32-filesystem",
        layout.primary_filesystem() == Some(FilesystemKind::Fat32),
        layout.primary_filesystem().map(|f| f.label()).unwrap_or("unknown").into(),
    );
    let label = layout.primary().and_then(|p| p.filesystem.as_ref()).and_then(|f| f.label.clone()).unwrap_or_default();
    check(checks, "role-label", label == "BR_TEST", label);
    Ok(())
}

fn run_copy_scenario(root: &Path, checks: &mut Vec<Check>) -> Result<()> {
    let usb = root.join("usb");
    create_stick(&usb, stick("samsung-64")?)?;
    create_stick(&usb, stick("kingston-32")?)?;
    let platform = boothready_platform::demo::DemoPlatform::new(usb);
    let devices = platform.list_devices()?;
    let source = devices.iter().find(|d| d.id == "demo:samsung-64").context("copy source fixture missing")?;
    let destination =
        devices.iter().find(|d| d.id == "demo:kingston-32").context("copy destination fixture missing")?;
    let src = source.primary_mount().and_then(|v| v.mount_point.clone()).context("source is not mounted")?;
    let dst = destination.primary_mount().and_then(|v| v.mount_point.clone()).context("destination is not mounted")?;
    let space = boothready_platform::space(&dst)?;
    let target = CopyTarget {
        filesystem: destination.primary_volume().and_then(|v| v.filesystem),
        free_bytes: space.free_bytes,
        cluster_bytes: space.cluster_bytes,
    };
    let plan = plan_copy(&src, &dst, &target)?;
    check(checks, "copy-plan", plan.can_start() && plan.files_to_copy > 0, format!("{} files", plan.files_to_copy));
    let report = run_copy(&plan, &src, &dst, &AtomicBool::new(false), |_| {})?;
    check(
        checks,
        "copy-complete",
        !report.cancelled && report.files_copied > 0,
        format!("{} files", report.files_copied),
    );
    let libs = boothready_core::library::scan_libraries(&dst);
    let verify = verify_volume(
        &VerifyRequest {
            root: &dst,
            layout: None,
            expect: Expectations { scheme: None, filesystem: None },
            libs: &libs,
            manifest: None,
            mode: boothready_core::manifest::VerifyMode::Full,
        },
        &AtomicBool::new(false),
        |_| {},
    );
    check(checks, "full-readback", verify.passed, format!("{} files checked", verify.files_checked));
    Ok(())
}

/// macOS clears the FAT32 clean-shutdown bit when it mounts a volume
/// read-write and sets it again on eject, so a raw read of a mounted stick
/// always sees it cleared. Observed on 2026-10-14 with `hdiutil attach` of a
/// `make-image` FAT32 image, before any file was written.
fn run_mounted_dirty_scenario(
    root: &Path,
    rules: &Ruleset,
    catalog: &UsbCatalog,
    checks: &mut Vec<Check>,
) -> Result<()> {
    let usb = root.join("usb");
    create_stick(&usb, stick("samsung-64")?)?;
    let platform = boothready_platform::demo::DemoPlatform::new(usb);
    let dev = platform
        .list_devices()?
        .into_iter()
        .find(|d| d.id == "demo:samsung-64")
        .context("fixture 'samsung-64' was not created")?;
    let size = 1u64 << 30;
    let mut img = fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&dev.os_path)?;
    img.set_len(size)?;
    let layout = build_fat32(&mut img, size, 512, "SAMSUNG", 0x6469_7274_7966)?;
    clear_fat32_clean_bit(&mut img, layout.partition_start)?;
    drop(img);

    let raw = inspect(fs::File::open(&dev.os_path)?)?;
    let raw_dirty = raw.primary().and_then(|p| p.filesystem.as_ref()).and_then(|f| f.dirty);
    check(checks, "raw-flag-cleared", raw_dirty == Some(true), format!("dirty {raw_dirty:?}"));

    let targets = rules.preset("pioneer_only").context("pioneer_only preset is missing")?;
    let devices = rules.devices_by_id(&targets.devices);
    let unsafe_eject = |a: &boothready_core::rules::DriveAssessment| {
        a.devices.iter().any(|d| d.layers.iter().any(|l| l.headline == "Not ejected safely last time"))
            || a.fixes.iter().any(|f| f.kind == FixKind::RepairFilesystem)
    };

    let mounted = analyze_drive(&dev, Some(raw.clone()), rules, catalog, |_, _| {});
    let assessment = assess_drive(&mounted.facts, &devices, rules);
    check(
        checks,
        "mounted-flag-ignored",
        mounted.mount_point.is_some() && mounted.facts.fs_dirty.is_none(),
        format!("mounted at {:?}, fs_dirty {:?}", mounted.mount_point, mounted.facts.fs_dirty),
    );
    check(checks, "mounted-no-eject-warning", !unsafe_eject(&assessment), assessment.overall.label().into());

    let mut unmounted_dev = dev.clone();
    for v in &mut unmounted_dev.volumes {
        v.mount_point = None;
    }
    let unmounted = analyze_drive(&unmounted_dev, Some(raw), rules, catalog, |_, _| {});
    let assessment = assess_drive(&unmounted.facts, &devices, rules);
    check(
        checks,
        "unmounted-flag-trusted",
        unmounted.facts.fs_dirty == Some(true) && unsafe_eject(&assessment),
        format!("fs_dirty {:?}, {}", unmounted.facts.fs_dirty, assessment.overall.label()),
    );
    Ok(())
}

/// Clear ClnShutBitMask in FAT[1] of every FAT copy.
fn clear_fat32_clean_bit(img: &mut fs::File, partition_start: u64) -> Result<()> {
    let mut boot = [0u8; 512];
    img.seek(SeekFrom::Start(partition_start))?;
    img.read_exact(&mut boot)?;
    let bps = u16::from_le_bytes([boot[11], boot[12]]) as u64;
    let reserved = u16::from_le_bytes([boot[14], boot[15]]) as u64;
    let fat_sectors = u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]) as u64;
    for i in 0..boot[16] as u64 {
        let at = partition_start + (reserved + i * fat_sectors) * bps + 4;
        let mut entry = [0u8; 4];
        img.seek(SeekFrom::Start(at))?;
        img.read_exact(&mut entry)?;
        let cleared = u32::from_le_bytes(entry) & !0x0800_0000;
        img.seek(SeekFrom::Start(at))?;
        img.write_all(&cleared.to_le_bytes())?;
    }
    Ok(())
}

fn stick(name: &str) -> Result<&'static DemoStick> {
    STICKS.iter().find(|s| s.name == name).with_context(|| format!("unknown fixture stick '{name}'"))
}

fn unreadable_count(report: &DriveReport) -> usize {
    report.audio.tracks.iter().filter(|t| matches!(t.probe, boothready_core::scan::TrackProbe::Unreadable(_))).count()
}

fn check(checks: &mut Vec<Check>, name: &str, passed: bool, detail: String) {
    checks.push(Check { name: name.into(), passed, detail });
}

/// Problems that prevent a mounted-volume metadata/header scan being complete.
pub fn read_issues(report: &DriveReport) -> Vec<String> {
    let Some(mount) = &report.mount_point else {
        return vec!["No mounted volume was available for a content scan.".into()];
    };
    let mut issues = Vec::new();
    for entry in walkdir::WalkDir::new(mount).follow_links(false) {
        if let Err(error) = entry {
            issues.push(format!("Could not inspect a directory entry: {error}"));
        }
    }
    if !Path::new(mount).is_dir() {
        issues.push("The mounted volume is no longer an accessible directory.".into());
    }
    let unreadable = unreadable_count(report);
    if unreadable > 0 {
        issues.push(format!("{unreadable} audio files could not be probed."));
    }
    issues
}

/// Atomically reserve a fresh report folder, preserving all earlier runs.
pub fn create_report_directory(parent: &Path, kind: &str) -> Result<PathBuf> {
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    for suffix in 0..1000 {
        let path = parent.join(format!("{kind}-{stamp}-{suffix}"));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("could not reserve a fresh test report directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_output_is_rejected_without_touching_contents() {
        let root = tempfile::tempdir().unwrap();
        let usb = root.path().join("empty-drive/usb");
        fs::create_dir_all(&usb).unwrap();
        let sentinel = usb.join("personal-file.txt");
        fs::write(&sentinel, "keep this").unwrap();
        let result = run_virtual_suite(
            Some("empty-drive"),
            false,
            Some(root.path().to_path_buf()),
            &Ruleset::builtin(),
            &UsbCatalog::builtin(),
        );
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "keep this");
        assert!(result.is_err());
    }

    #[test]
    fn missing_mount_is_an_incomplete_read() {
        let root = tempfile::tempdir().unwrap();
        create_stick(root.path(), stick("kingston-32").unwrap()).unwrap();
        let platform = boothready_platform::demo::DemoPlatform::new(root.path().to_path_buf());
        let mut dev = platform.list_devices().unwrap().remove(0);
        dev.volumes.clear();
        let report = analyze_drive(&dev, None, &Ruleset::builtin(), &UsbCatalog::builtin(), |_, _| {});
        assert!(!read_issues(&report).is_empty());
    }

    #[test]
    fn removed_mount_is_an_incomplete_read_but_empty_mounted_volume_passes() {
        let root = tempfile::tempdir().unwrap();
        create_stick(root.path(), stick("kingston-32").unwrap()).unwrap();
        let platform = boothready_platform::demo::DemoPlatform::new(root.path().to_path_buf());
        let dev = platform.list_devices().unwrap().remove(0);
        let report = analyze_drive(&dev, None, &Ruleset::builtin(), &UsbCatalog::builtin(), |_, _| {});
        assert!(read_issues(&report).is_empty());
        fs::remove_dir_all(report.mount_point.as_ref().unwrap()).unwrap();
        assert!(!read_issues(&report).is_empty());
    }

    #[test]
    fn repeated_reports_keep_separate_outputs() {
        let root = tempfile::tempdir().unwrap();
        let first = create_report_directory(root.path(), "real").unwrap();
        fs::write(first.join("report.json"), "original").unwrap();
        let second = create_report_directory(root.path(), "real").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first.join("report.json")).unwrap(), "original");
    }

    #[test]
    fn complete_virtual_matrix_passes() {
        let root = tempfile::tempdir().unwrap();
        let suite = run_virtual_suite(
            None,
            false,
            Some(root.path().join("reports")),
            &Ruleset::builtin(),
            &UsbCatalog::builtin(),
        )
        .unwrap();
        assert!(suite.passed, "{suite:?}");
        assert_eq!(suite.scenarios.len(), if cfg!(windows) { 5 } else { 6 });
        assert!(suite.scenarios.iter().all(|s| s.artifact.is_some()));
    }
}
