//! Repeatable virtual scenarios used by the CLI and the desktop test-lab UI.
//!
//! Every write in this module is scoped to a fixture directory. The module
//! never opens a native disk and never prepares, copies or ejects a real USB.

use anyhow::{bail, Context, Result};
use boothready_core::copy::{plan_copy, run_copy, CopyTarget};
use boothready_core::demo::{create_stick, DemoStick, STICKS};
use boothready_core::drive::{analyze_drive, DriveReport};
use boothready_core::identify::UsbCatalog;
use boothready_core::library::LibraryFormat;
use boothready_core::media::inspect;
use boothready_core::privileged::{DeviceFingerprint, HelperEvent, HelperRequest};
use boothready_core::rules::{assess_drive, Ruleset, Verdict};
use boothready_core::verify::{verify_volume, Expectations, VerifyRequest};
use boothready_helper::{handle, Backend, DemoBackend};
use boothready_model::{FilesystemKind, PartitionScheme};
use boothready_platform::Platform;
use serde::Serialize;
use std::fs;
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

pub fn scenarios() -> Vec<ScenarioInfo> {
    SCENARIOS
        .iter()
        .map(|s| ScenarioInfo { id: s.id.into(), title: s.title.into(), description: s.description.into() })
        .collect()
}

/// Run one scenario or the complete matrix. `root` is the report directory;
/// if omitted, a timestamped `test-reports/virtual-*` directory is used.
pub fn run_virtual_suite(
    selected: Option<&str>,
    keep_fixtures: bool,
    root: Option<PathBuf>,
    rules: &Ruleset,
    catalog: &UsbCatalog,
) -> Result<VirtualSuite> {
    let selected: Vec<&Scenario> = match selected {
        None | Some("all") => SCENARIOS.iter().collect(),
        Some(id) => {
            vec![SCENARIOS.iter().find(|s| s.id == id).with_context(|| format!("unknown virtual scenario '{id}'"))?]
        }
    };
    let root = root.unwrap_or_else(|| PathBuf::from("test-reports").join(format!("virtual-{}", timestamp())));
    fs::create_dir_all(&root)?;

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

fn stick(name: &str) -> Result<&'static DemoStick> {
    STICKS.iter().find(|s| s.name == name).with_context(|| format!("unknown fixture stick '{name}'"))
}

fn unreadable_count(report: &DriveReport) -> usize {
    report.audio.tracks.iter().filter(|t| matches!(t.probe, boothready_core::scan::TrackProbe::Unreadable(_))).count()
}

fn check(checks: &mut Vec<Check>, name: &str, passed: bool, detail: String) {
    checks.push(Check { name: name.into(), passed, detail });
}

fn timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(suite.scenarios.len(), 5);
        assert!(suite.scenarios.iter().all(|s| s.artifact.is_some()));
    }
}
