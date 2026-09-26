//! `boothready`: the command-line front end.

mod demo;
mod render;

use anyhow::{bail, Context, Result};
use boothready_core::drive::{analyze_drive, DriveReport};
use boothready_core::identify::UsbCatalog;
use boothready_core::library::scan_libraries;
use boothready_core::manifest::{read_manifest, write_manifest, Manifest, PreparationState, VerifyMode};
use boothready_core::media::inspect_path;
use boothready_core::planner::{plan_kit, DriveCandidate, KitRequest, GB};
use boothready_core::privileged::{DeviceFingerprint, HelperEvent, HelperRequest};
use boothready_core::rules::{assess_drive, DeviceProfile, Ruleset};
use boothready_core::verify::{verify_volume, Expectations, VerifyRequest};
use boothready_helper::{Backend, DemoBackend, NativeBackend};
use boothready_model::{FilesystemKind, PartitionScheme, PhysicalDevice};
use boothready_platform::Platform;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[derive(Parser)]
#[command(name = "boothready", version, about = "Plug in your USB. We'll make sure you're ready to play.")]
struct Cli {
    /// Use a folder of simulated USB drives instead of real hardware.
    #[arg(long, global = true, value_name = "DIR")]
    demo: Option<PathBuf>,
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Targets {
    /// Target preset: unknown_club, pioneer_only, denon_engine, laptop_mixxx, mixed, max_compat.
    #[arg(long, default_value = "unknown_club")]
    preset: String,
    /// Explicit device IDs (comma separated); overrides --preset.
    #[arg(long, value_delimiter = ',')]
    targets: Vec<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List storage devices and whether BoothReady may prepare them.
    Devices,
    /// Identify, scan and assess a connected drive.
    Check {
        device: String,
        #[command(flatten)]
        targets: Targets,
    },
    /// Assess a mounted folder (and optionally its raw image) without a device.
    CheckDir {
        dir: PathBuf,
        /// Raw image of the drive, for partition-level checks.
        #[arg(long)]
        image: Option<PathBuf>,
        /// Partition scheme to assume when there's no image (mbr, gpt).
        #[arg(long)]
        scheme: Option<String>,
        /// Filesystem to assume when there's no image (fat32, exfat, ...).
        #[arg(long)]
        fs: Option<String>,
        #[command(flatten)]
        targets: Targets,
    },
    /// Show the partition table and filesystems of an image or device node.
    Inspect { path: PathBuf },
    /// Plan a multi-USB gig kit.
    Plan {
        #[command(flatten)]
        targets: Targets,
        #[arg(long)]
        library_gb: f64,
        #[arg(long)]
        essential_gb: Option<f64>,
        /// Available drive as NAME:VENDOR:GB (repeatable).
        #[arg(long = "drive")]
        drives: Vec<String>,
    },
    /// Verify a mounted drive. Full mode reads every audio file.
    Verify {
        dir: PathBuf,
        #[arg(long)]
        full: bool,
        /// Record the result in the drive's BoothReady manifest.
        #[arg(long)]
        save: bool,
    },
    /// Erase and prepare a drive (demo drives, or real ones when run as admin).
    Prepare {
        device: String,
        #[arg(long, default_value = "fat32")]
        fs: String,
        #[arg(long, default_value = "BR_MAIN")]
        label: String,
        /// Skip the interactive confirmation (demo drives only).
        #[arg(long)]
        yes: bool,
    },
    /// Safely eject a drive.
    Eject { device: String },
    /// Build an MBR + FAT32 disk image (never touches devices).
    MakeImage {
        path: PathBuf,
        #[arg(long)]
        size_gb: f64,
        #[arg(long, default_value = "BR_MAIN")]
        label: String,
    },
    /// Browse the compatibility rules.
    #[command(subcommand)]
    Rules(RulesCmd),
    /// Create and manage simulated USB drives for demos.
    #[command(subcommand)]
    Demo(demo::DemoCmd),
}

#[derive(Subcommand)]
enum RulesCmd {
    Presets,
    Search {
        query: String,
    },
    Device {
        id: String,
    },
    /// Explain the recommended format for a set of targets.
    Why {
        #[command(flatten)]
        targets: Targets,
    },
}

fn resolve_targets<'a>(rules: &'a Ruleset, t: &Targets) -> Result<Vec<&'a DeviceProfile>> {
    let ids: Vec<String> = if t.targets.is_empty() {
        rules.preset(&t.preset).with_context(|| format!("unknown preset '{}'", t.preset))?.devices.clone()
    } else {
        t.targets.clone()
    };
    let devs = rules.devices_by_id(&ids);
    if devs.len() != ids.len() {
        let known: Vec<&str> = devs.iter().map(|d| d.id.as_str()).collect();
        let missing: Vec<&String> = ids.iter().filter(|i| !known.contains(&i.as_str())).collect();
        bail!("unknown device id(s): {missing:?}. Try `boothready rules search`.");
    }
    Ok(devs)
}

fn backend(cli: &Cli) -> Box<dyn Backend> {
    match &cli.demo {
        Some(dir) => Box::new(DemoBackend::new(dir.join("usb"))),
        None => Box::new(NativeBackend::new()),
    }
}

fn find_device(platform: &dyn Platform, id: &str) -> Result<PhysicalDevice> {
    platform
        .list_devices()?
        .into_iter()
        .find(|d| d.id == id || d.id == format!("demo:{id}"))
        .with_context(|| format!("no device '{id}'. Run `boothready devices`."))
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let rules = Ruleset::builtin();
    let catalog = UsbCatalog::builtin();
    match &cli.cmd {
        Cmd::Devices => {
            let b = backend(&cli);
            let devices = b.platform().list_devices()?;
            if cli.json {
                return print_json(&devices);
            }
            render::devices(&devices, &catalog);
        }
        Cmd::Check { device, targets } => {
            let b = backend(&cli);
            let dev = find_device(b.platform(), device)?;
            let raw = b.open_read(&dev).ok().and_then(|f| boothready_core::media::inspect(f).ok());
            let report = analyze_drive(&dev, raw, &rules, &catalog, |_, _| {});
            report_and_assess(&cli, &report, &rules, targets)?;
        }
        Cmd::CheckDir { dir, image, scheme, fs, targets } => {
            let mut dev = demo::synthetic_device(dir, scheme.as_deref(), fs.as_deref())?;
            if let Some(v) = dev.volumes.first_mut() {
                v.mount_point = Some(dir.clone());
            }
            let raw = image.as_ref().map(|p| inspect_path(p)).transpose()?;
            let report = analyze_drive(&dev, raw, &rules, &catalog, |_, _| {});
            report_and_assess(&cli, &report, &rules, targets)?;
        }
        Cmd::Inspect { path } => {
            let layout = inspect_path(path).with_context(|| format!("reading {}", path.display()))?;
            if cli.json {
                return print_json(&layout);
            }
            render::layout(&layout);
        }
        Cmd::Plan { targets, library_gb, essential_gb, drives } => {
            let devs = resolve_targets(&rules, targets)?;
            let redundancy = targets.targets.is_empty() && rules.preset(&targets.preset).is_some_and(|p| p.redundancy);
            let drives = drives
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let parts: Vec<&str> = s.split(':').collect();
                    let [name, vendor, gb] = parts[..] else {
                        bail!("--drive must look like NAME:VENDOR:GB, got '{s}'")
                    };
                    Ok(DriveCandidate {
                        id: format!("drive{}", i + 1),
                        name: name.to_string(),
                        vendor: Some(vendor.to_string()),
                        capacity_bytes: (gb.parse::<f64>()? * GB as f64) as u64,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let req = KitRequest {
                targets: devs.iter().map(|d| d.id.clone()).collect(),
                redundancy,
                library_bytes: (library_gb * GB as f64) as u64,
                essential_bytes: essential_gb.map(|g| (g * GB as f64) as u64),
                drives,
            };
            let plan = plan_kit(&req, &rules);
            if cli.json {
                return print_json(&plan);
            }
            render::plan(&plan, &req.drives);
        }
        Cmd::Verify { dir, full, save } => {
            let libs = scan_libraries(dir);
            let manifest = read_manifest(dir);
            let mode = if *full { VerifyMode::Full } else { VerifyMode::Quick };
            let req = VerifyRequest {
                root: dir,
                layout: None,
                expect: Expectations { scheme: None, filesystem: None },
                libs: &libs,
                manifest: manifest.as_ref(),
                mode,
            };
            let cancel = AtomicBool::new(false);
            let quiet = cli.json;
            let report = verify_volume(&req, &cancel, |p| {
                if !quiet && p.bytes_total > 0 {
                    eprint!("\r{:.1} of {:.1} MB verified   ", p.bytes_done as f64 / 1e6, p.bytes_total as f64 / 1e6);
                }
            });
            if !quiet {
                eprintln!();
            }
            if *save {
                let mut m = manifest.unwrap_or_else(|| Manifest::new(None, vec![]));
                if m.is_interrupted() && report.passed {
                    m.preparation = PreparationState::Complete { finished_unix: report.completed_unix };
                }
                m.verification = Some(report.record());
                write_manifest(dir, &m)?;
            }
            if cli.json {
                return print_json(&report);
            }
            render::verification(&report);
            if !report.passed {
                std::process::exit(1);
            }
        }
        Cmd::Prepare { device, fs, label, yes } => prepare(&cli, device, fs, label, *yes)?,
        Cmd::Eject { device } => {
            let b = backend(&cli);
            let dev = find_device(b.platform(), device)?;
            match b.platform().eject(&dev.id) {
                Ok(()) => println!("Safe to remove {}.", dev.raw_display_name()),
                Err(e) => bail!("{e}"),
            }
        }
        Cmd::MakeImage { path, size_gb, label } => {
            if path.exists() && !std::fs::metadata(path)?.is_file() {
                bail!("{} is not a regular file; make-image never writes to devices", path.display());
            }
            let size = (size_gb * 1024.0 * 1024.0 * 1024.0) as u64;
            let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
            f.set_len(size)?;
            let mut f = f;
            let layout = boothready_core::format::build_fat32(&mut f, size, 512, label, 0x6A6F_6D6D_6939)?;
            println!(
                "Created {} ({} + FAT32, {} KB clusters, label {})",
                path.display(),
                layout.scheme.label(),
                layout.cluster_size / 1024,
                label.to_uppercase()
            );
        }
        Cmd::Rules(r) => match r {
            RulesCmd::Presets => {
                if cli.json {
                    return print_json(&rules.presets);
                }
                for p in &rules.presets {
                    println!("{:<14} {}  ({})", p.id, p.title, p.subtitle);
                }
            }
            RulesCmd::Search { query } => {
                let found = rules.search(query);
                if cli.json {
                    return print_json(&found);
                }
                for d in found {
                    println!("{:<20} {} {} ({})", d.id, d.manufacturer, d.model, d.released);
                }
            }
            RulesCmd::Device { id } => {
                let d = rules.device(id).with_context(|| format!("unknown device '{id}'"))?;
                print_json(d)?;
            }
            RulesCmd::Why { targets } => {
                let devs = resolve_targets(&rules, targets)?;
                let rec = boothready_core::rules::recommend_format(&devs);
                if cli.json {
                    return print_json(&rec);
                }
                println!("Recommended: {} + {}\n\n{}", rec.scheme.label(), rec.filesystem.label(), rec.why);
            }
        },
        Cmd::Demo(d) => demo::run(d, &cli.demo)?,
    }
    Ok(())
}

fn report_and_assess(cli: &Cli, report: &DriveReport, rules: &Ruleset, targets: &Targets) -> Result<()> {
    let devs = resolve_targets(rules, targets)?;
    let assessment = assess_drive(&report.facts, &devs, rules);
    if cli.json {
        #[derive(serde::Serialize)]
        struct Out<'a> {
            drive: &'a DriveReport,
            assessment: &'a boothready_core::rules::DriveAssessment,
        }
        return print_json(&Out { drive: report, assessment: &assessment });
    }
    render::report(report);
    render::assessment(&assessment);
    Ok(())
}

fn prepare(cli: &Cli, device: &str, fs: &str, label: &str, yes: bool) -> Result<()> {
    let b = backend(cli);
    let dev = find_device(b.platform(), device)?;
    let filesystem =
        FilesystemKind::from_name(fs).filter(|f| matches!(f, FilesystemKind::Fat32 | FilesystemKind::Exfat));
    let Some(filesystem) = filesystem else { bail!("--fs must be fat32 or exfat") };
    let catalog = UsbCatalog::builtin();
    let id = boothready_core::identify::identify(&dev, &catalog);
    let fp = DeviceFingerprint::of(&dev);
    let elig = boothready_core::privileged::eligibility(&dev);
    if !elig.eligible {
        bail!("BoothReady won't erase this drive: {}", elig.reasons.join(" "));
    }
    let stats =
        dev.primary_mount().and_then(|v| v.mount_point.as_ref()).map(|p| boothready_core::drive::content_stats(p));
    eprintln!("You are about to erase:\n");
    eprintln!("  {}", id.display_name);
    eprintln!("  {:.1} GB", dev.size_bytes as f64 / 1e9);
    if let Some(l) = dev.primary_volume().and_then(|v| v.label.as_ref()) {
        eprintln!("  Volume: {l}");
    }
    if let Some(tail) = fp.serial_tail() {
        eprintln!("  Serial ending: {tail}");
    }
    if let Some(s) = &stats {
        eprintln!("  {} files, {:.1} GB used", s.files, s.used_bytes as f64 / 1e9);
    }
    eprintln!("\nIt will be rebuilt as MBR + {} labelled {}.", filesystem.label(), label.to_uppercase());
    let is_demo = cli.demo.is_some();
    if !(yes && is_demo) {
        let expect = fp.serial_tail().unwrap_or_else(|| "ERASE".into());
        eprint!("Type {expect} to erase this drive: ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim().to_uppercase() != expect {
            bail!("Not confirmed. Nothing was erased.");
        }
    }
    let req = HelperRequest::Prepare {
        confirmation: fp.token(),
        expected: fp,
        scheme: PartitionScheme::Mbr,
        filesystem,
        label: label.to_string(),
    };
    let mut failed = None;
    boothready_helper::handle(b.as_ref(), req, &mut |e| match e {
        HelperEvent::Progress { detail, .. } => eprintln!("  {detail}…"),
        HelperEvent::Done { .. } => eprintln!("Done. {} is now MBR + {}.", id.display_name, filesystem.label()),
        HelperEvent::Error { message, .. } => failed = Some(message),
        _ => {}
    });
    if let Some(m) = failed {
        bail!(m);
    }
    Ok(())
}
