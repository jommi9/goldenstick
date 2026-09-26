//! Simulated USB drives for demos and UI work.

use anyhow::{bail, Context, Result};
use boothready_core::demo::{populate, DemoContent};
use boothready_model::{BusType, FilesystemKind, PartitionScheme, PhysicalDevice, Volume};
use boothready_platform::demo::DemoPlatform;
use clap::Subcommand;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum DemoCmd {
    /// Create three simulated drives in <DIR>/available (use with --demo DIR).
    Init,
    /// "Plug in" a simulated drive.
    Insert { name: String },
    /// "Pull out" a simulated drive.
    Remove { name: String },
    /// Show which simulated drives exist and which are plugged in.
    List,
}

pub struct DemoStick {
    pub name: &'static str,
    pub maker: &'static str,
    pub product: &'static str,
    pub vid: u16,
    pub pid: u16,
    pub size: u64,
    pub scheme: PartitionScheme,
    pub fs: FilesystemKind,
    pub label: &'static str,
    pub content: DemoContent,
}

pub const STICKS: &[DemoStick] = &[
    DemoStick {
        name: "sandisk-128",
        maker: "SanDisk",
        product: "Ultra",
        vid: 0x0781,
        pid: 0x5581,
        size: 123_060_000_000,
        scheme: PartitionScheme::Gpt,
        fs: FilesystemKind::Exfat,
        label: "FESTIVAL26",
        content: DemoContent::DeviceLibraryWithIssues,
    },
    DemoStick {
        name: "kingston-32",
        maker: "Kingston",
        product: "DataTraveler 3.0",
        vid: 0x0951,
        pid: 0x1666,
        size: 30_752_000_000,
        scheme: PartitionScheme::Gpt,
        fs: FilesystemKind::Exfat,
        label: "UNTITLED",
        content: DemoContent::Empty,
    },
    DemoStick {
        name: "samsung-64",
        maker: "Samsung",
        product: "Flash Drive BAR Plus",
        vid: 0x090c,
        pid: 0x1000,
        size: 64_023_000_000,
        scheme: PartitionScheme::Mbr,
        fs: FilesystemKind::Fat32,
        label: "BR_BACKUP",
        content: DemoContent::BothLibraries,
    },
];

/// Create one simulated stick under `root`.
pub fn create(root: &Path, s: &DemoStick) -> Result<PathBuf> {
    let dir = DemoPlatform::create_stick(root, s.name, s.maker, s.product, s.vid, s.pid, s.size)?;
    let path = dir.join("device.json");
    let mut dev: PhysicalDevice = serde_json::from_str(&fs::read_to_string(&path)?)?;
    dev.partition_scheme = Some(s.scheme);
    if let Some(v) = dev.volumes.first_mut() {
        v.filesystem = Some(s.fs);
        v.label = Some(s.label.to_string());
        if s.scheme == PartitionScheme::Gpt {
            // macOS-style GPT: 200 MB EFI partition first.
            v.offset_bytes = Some(209_735_680);
        }
    }
    fs::write(&path, serde_json::to_vec_pretty(&dev)?)?;
    populate(&dir.join("volume"), s.content)?;
    Ok(dir)
}

pub fn run(cmd: &DemoCmd, demo: &Option<PathBuf>) -> Result<()> {
    let Some(root) = demo else { bail!("demo commands need --demo <DIR>") };
    let available = root.join("available");
    let usb = root.join("usb");
    match cmd {
        DemoCmd::Init => {
            fs::create_dir_all(&available)?;
            fs::create_dir_all(&usb)?;
            for s in STICKS {
                if !available.join(s.name).exists() && !usb.join(s.name).exists() {
                    create(&available, s)?;
                }
            }
            println!("Created simulated drives in {}:", available.display());
            for s in STICKS {
                println!("  {:<12} {} {} ({} GB)", s.name, s.maker, s.product, s.size / 1_000_000_000);
            }
            println!("\nPlug one in:  boothready --demo {} demo insert sandisk-128", root.display());
        }
        DemoCmd::Insert { name } => {
            fs::create_dir_all(&usb)?;
            fs::rename(available.join(name), usb.join(name))
                .with_context(|| format!("no simulated drive '{name}' waiting in {}", available.display()))?;
            let _ = fs::remove_file(usb.join(name).join(".ejected"));
            let _ = fs::remove_file(usb.join(name).join(".unmounted"));
            println!("Inserted {name}.");
        }
        DemoCmd::Remove { name } => {
            fs::rename(usb.join(name), available.join(name)).with_context(|| format!("'{name}' isn't plugged in"))?;
            println!("Removed {name}.");
        }
        DemoCmd::List => {
            for (state, dir) in [("waiting", &available), ("plugged in", &usb)] {
                if let Ok(rd) = fs::read_dir(dir) {
                    for e in rd.flatten() {
                        println!("{:<12} {}", e.file_name().to_string_lossy(), state);
                    }
                }
            }
        }
    }
    Ok(())
}

/// A stand-in device for `check-dir`, which assesses a plain folder.
pub fn synthetic_device(dir: &Path, scheme: Option<&str>, fs: Option<&str>) -> Result<PhysicalDevice> {
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let scheme = match scheme.map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("mbr") => Some(PartitionScheme::Mbr),
        Some("gpt") => Some(PartitionScheme::Gpt),
        Some(o) => bail!("unknown scheme '{o}' (mbr or gpt)"),
    };
    let fs = match fs {
        None => None,
        Some(f) => Some(FilesystemKind::from_name(f).with_context(|| format!("unknown filesystem '{f}'"))?),
    };
    Ok(PhysicalDevice {
        id: "folder".into(),
        os_path: dir.to_string_lossy().into_owned(),
        bus: BusType::Usb,
        removable: true,
        is_system: false,
        size_bytes: 0,
        logical_sector_size: 512,
        storage_vendor: None,
        storage_model: Some(dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        storage_revision: None,
        usb: None,
        partition_scheme: scheme,
        volumes: vec![Volume {
            os_path: dir.to_string_lossy().into_owned(),
            mount_point: Some(dir.to_path_buf()),
            label: None,
            filesystem: fs,
            size_bytes: 0,
            offset_bytes: Some(1 << 20),
            uuid: None,
        }],
    })
}
