//! BoothReady's repeatable test lab.
//!
//! The virtual matrix lives in `boothready-testlab` so the CLI and desktop
//! GUI exercise the same scenarios. The real suite is intentionally
//! read-only: it records diagnostics, reads one named USB and writes a report
//! on the computer. Preparation, copying and ejecting real devices stay
//! behind their existing confirmation flows.

use anyhow::{bail, Context, Result};
use boothready_core::drive::{analyze_drive, DriveReport};
use boothready_core::identify::UsbCatalog;
use boothready_core::media::inspect;
use boothready_core::rules::{assess_drive, DriveAssessment, Ruleset};
use boothready_helper::{Backend, NativeBackend};
use boothready_model::PhysicalDevice;
use boothready_platform::diagnostics::{self, Diagnostics};
use clap::{Args, Subcommand};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Subcommand)]
pub enum TestCmd {
    /// List the virtual scenarios available in the test lab.
    List,
    /// Run one virtual scenario, or the complete virtual matrix when omitted.
    Virtual(VirtualArgs),
    /// Read one real USB without changing it and save diagnostics plus a report.
    Real(RealArgs),
    /// Replay a diagnostics report and compare the rebuilt device list.
    Replay { report: PathBuf },
}

#[derive(Args)]
pub struct VirtualArgs {
    /// Scenario ID. Omit it to run every scenario.
    pub scenario: Option<String>,
    /// Keep fixture contents after the report is written.
    #[arg(long)]
    pub keep: bool,
    /// Root directory for retained fixtures and reports.
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

#[derive(Args)]
pub struct RealArgs {
    /// Exact device ID from `boothready devices`.
    pub device: String,
    #[command(flatten)]
    pub targets: super::Targets,
    /// Directory for diagnostics-before.json, diagnostics-after.json and report.json.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct RealResult<'a> {
    kind: &'static str,
    read_ok: bool,
    device: &'a PhysicalDevice,
    drive: &'a DriveReport,
    assessment: &'a DriveAssessment,
    diagnostics_before: String,
    diagnostics_after: String,
    report: String,
}

#[derive(Debug, Serialize)]
struct ReplayResult {
    kind: &'static str,
    supported: bool,
    matched_recorded_devices: bool,
    recorded_devices: usize,
    rebuilt_devices: usize,
    error: Option<String>,
}

pub fn run(cmd: &TestCmd, cli: &super::Cli, rules: &Ruleset, catalog: &UsbCatalog) -> Result<()> {
    match cmd {
        TestCmd::List => list(cli.json),
        TestCmd::Virtual(args) => virtual_suite(args, cli.json, rules, catalog),
        TestCmd::Real(args) => real_test(args, cli, rules, catalog),
        TestCmd::Replay { report } => replay(report, cli.json),
    }
}

fn list(json: bool) -> Result<()> {
    let items = boothready_testlab::scenarios();
    if json {
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        for s in &items {
            println!("{:<22} {}\n  {}", s.id, s.title, s.description);
        }
    }
    Ok(())
}

fn virtual_suite(args: &VirtualArgs, json: bool, rules: &Ruleset, catalog: &UsbCatalog) -> Result<()> {
    let suite = boothready_testlab::run_virtual_suite(
        args.scenario.as_deref(),
        args.keep || args.root.is_some(),
        args.root.clone(),
        rules,
        catalog,
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&suite)?);
    } else {
        println!("Virtual test lab: {}", if suite.passed { "PASS" } else { "FAIL" });
        for s in &suite.scenarios {
            println!("  {:<22} {} ({} ms)", s.id, if s.passed { "PASS" } else { "FAIL" }, s.duration_ms);
            for c in &s.checks {
                println!("    {} {:<28} {}", if c.passed { "PASS" } else { "FAIL" }, c.name, c.detail);
            }
            if let Some(e) = &s.error {
                println!("    ERROR {e}");
            }
        }
        println!("Reports: {}", suite.root);
    }
    if suite.passed {
        Ok(())
    } else {
        bail!("one or more virtual scenarios failed")
    }
}

fn real_test(args: &RealArgs, cli: &super::Cli, rules: &Ruleset, catalog: &UsbCatalog) -> Result<()> {
    if cli.demo.is_some() {
        bail!("`test real` uses the native platform; omit --demo and use `test virtual` for fixtures");
    }
    let out = args.out.clone().unwrap_or_else(|| PathBuf::from("test-reports").join(format!("real-{}", timestamp())));
    fs::create_dir_all(&out)?;
    let backend = NativeBackend::new();
    let before = diagnostics::collect(backend.platform(), env!("CARGO_PKG_VERSION"));
    let before_path = out.join("diagnostics-before.json");
    write_json(&before_path, &before)?;
    let dev = backend
        .platform()
        .list_devices()?
        .into_iter()
        .find(|d| d.id == args.device)
        .with_context(|| format!("no exact device ID '{}'; run `boothready devices`", args.device))?;
    if dev.is_system {
        bail!("refusing to test system device {}", dev.id);
    }
    if dev.bus != boothready_model::BusType::Usb {
        bail!("refusing to test non-USB device {}", dev.id);
    }
    if !cli.json {
        println!("Read-only USB test");
        println!("  ID: {}", dev.id);
        println!("  Model: {}", dev.raw_display_name());
        println!("  Size: {:.1} GB", dev.size_bytes as f64 / 1e9);
        println!("  No preparation, copy or eject will be attempted.");
    }
    let raw = backend.open_read(&dev).ok().and_then(|f| inspect(f).ok());
    let drive = analyze_drive(&dev, raw, rules, catalog, |_, _| {});
    let targets = super::resolve_targets(rules, &args.targets)?;
    let assessment = assess_drive(&drive.facts, &targets, rules);
    let after = diagnostics::collect(backend.platform(), env!("CARGO_PKG_VERSION"));
    let after_path = out.join("diagnostics-after.json");
    write_json(&after_path, &after)?;
    let report_path = out.join("report.json");
    let result = RealResult {
        kind: "real_read_only",
        read_ok: true,
        device: &dev,
        drive: &drive,
        assessment: &assessment,
        diagnostics_before: before_path.display().to_string(),
        diagnostics_after: after_path.display().to_string(),
        report: report_path.display().to_string(),
    };
    write_json(&report_path, &result)?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("  Verdict: {}", assessment.overall.label());
        println!("  Before: {}", before_path.display());
        println!("  After:  {}", after_path.display());
        println!("  Report: {}", report_path.display());
    }
    Ok(())
}

fn replay(path: &Path, json: bool) -> Result<()> {
    let report: Diagnostics =
        serde_json::from_slice(&fs::read(path)?).with_context(|| format!("reading {}", path.display()))?;
    let result = match diagnostics::replay(&report) {
        None => ReplayResult {
            kind: "diagnostics_replay",
            supported: false,
            matched_recorded_devices: false,
            recorded_devices: report.devices.len(),
            rebuilt_devices: 0,
            error: Some(format!("{} diagnostics replay is not supported", report.os)),
        },
        Some(Ok(devices)) => ReplayResult {
            kind: "diagnostics_replay",
            supported: true,
            matched_recorded_devices: devices == report.devices,
            recorded_devices: report.devices.len(),
            rebuilt_devices: devices.len(),
            error: None,
        },
        Some(Err(e)) => ReplayResult {
            kind: "diagnostics_replay",
            supported: true,
            matched_recorded_devices: false,
            recorded_devices: report.devices.len(),
            rebuilt_devices: 0,
            error: Some(e.to_string()),
        },
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Diagnostics replay: {}", if result.matched_recorded_devices { "PASS" } else { "UNVERIFIED" });
        println!("  Recorded devices: {}", result.recorded_devices);
        println!("  Rebuilt devices:  {}", result.rebuilt_devices);
        if let Some(e) = result.error {
            println!("  {e}");
        }
    }
    if result.supported && !result.matched_recorded_devices {
        bail!("diagnostics replay did not match the recorded devices")
    }
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}
