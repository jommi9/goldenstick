//! Realistic demo drive contents: a rekordbox-style export with real audio
//! headers (tiny files), playlists, and the usual problems DJs run into.
//! Used by the CLI's `demo` command and the app's demo mode.

use crate::audio::fixtures as audio;
use crate::library::pdb_fixture::{build_pdb, FixturePlaylist, FixtureTrack};
use crate::library::{DEVICE_LIBRARY_FILE, ONE_LIBRARY_FILE};
use boothready_model::{BusType, FilesystemKind, PartitionScheme, PhysicalDevice, UsbDescriptor, Volume};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoContent {
    /// Device Library only, with hi-res FLAC, an unreadable file and macOS
    /// litter. The PRD's "USB A".
    DeviceLibraryWithIssues,
    /// Device Library and OneLibrary, all tracks legacy-safe.
    BothLibraries,
    /// A clean, complete export from current rekordbox: both databases,
    /// hi-res tracks included, no litter.
    FullExport,
    Empty,
}

struct Tr {
    path: &'static str,
    title: &'static str,
    artist: u32,
    bytes: Vec<u8>,
    rate: u32,
    depth: u16,
}

fn tracks(with_issues: bool) -> Vec<Tr> {
    let mut v = vec![
        Tr {
            path: "Contents/Kolsch/Speicher/Grey.mp3",
            title: "Grey",
            artist: 1,
            bytes: audio::mp3_cbr(120),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Kolsch/Speicher/Goldfisch.mp3",
            title: "Goldfisch",
            artist: 1,
            bytes: audio::mp3_cbr(140),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Charlotte de Witte/Selected/Doppler.aiff",
            title: "Doppler",
            artist: 2,
            bytes: audio::aiff(44_100, 16, 2, 22_050, false),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Charlotte de Witte/Selected/Sgadi Li Mi.wav",
            title: "Sgadi Li Mi",
            artist: 2,
            bytes: audio::wav(48_000, 24, 2, 24_000, false),
            rate: 48_000,
            depth: 24,
        },
        Tr {
            path: "Contents/Ben Klock/Subzero/Subzero.aiff",
            title: "Subzero",
            artist: 3,
            bytes: audio::aiff(44_100, 24, 2, 22_050, false),
            rate: 44_100,
            depth: 24,
        },
        Tr {
            path: "Contents/Ben Klock/Subzero/Compression.m4a",
            title: "Compression",
            artist: 3,
            bytes: audio::m4a("aac-lc", 44_100, 360),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Paula Temple/Colonized/Colonized.wav",
            title: "Colonized",
            artist: 4,
            bytes: audio::wav(44_100, 16, 2, 22_050, false),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Paula Temple/Colonized/Deathvox.mp3",
            title: "Deathvox",
            artist: 4,
            bytes: audio::mp3_cbr(100),
            rate: 44_100,
            depth: 16,
        },
    ];
    if with_issues {
        v.push(Tr {
            path: "Contents/Hi-Res/Ambient Intro.flac",
            title: "Ambient Intro",
            artist: 5,
            bytes: audio::flac(96_000, 24, 2, 96_000 * 400),
            rate: 96_000,
            depth: 24,
        });
        v.push(Tr {
            path: "Contents/Hi-Res/Sunrise Edit.flac",
            title: "Sunrise Edit",
            artist: 5,
            bytes: audio::flac(44_100, 16, 2, 44_100 * 300),
            rate: 44_100,
            depth: 16,
        });
        v.push(Tr {
            path: "Contents/Hi-Res/Studio Master.wav",
            title: "Studio Master",
            artist: 5,
            bytes: audio::wav(96_000, 24, 2, 9_600, true),
            rate: 96_000,
            depth: 24,
        });
        v.push(Tr {
            path: "Contents/Hi-Res/Vinyl Rip.m4a",
            title: "Vinyl Rip",
            artist: 5,
            bytes: audio::m4a("alac", 44_100, 280),
            rate: 44_100,
            depth: 16,
        });
    }
    v
}

fn write(root: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let p = root.join(rel);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(p, bytes)
}

pub fn populate(root: &Path, content: DemoContent) -> io::Result<()> {
    fs::create_dir_all(root)?;
    if content == DemoContent::Empty {
        return Ok(());
    }
    let hires = matches!(content, DemoContent::DeviceLibraryWithIssues | DemoContent::FullExport);
    let litter = content == DemoContent::DeviceLibraryWithIssues;
    let one_library = matches!(content, DemoContent::BothLibraries | DemoContent::FullExport);
    let list = tracks(hires);
    let mut fixture = Vec::new();
    for (i, t) in list.iter().enumerate() {
        write(root, t.path, &t.bytes)?;
        fixture.push(FixtureTrack {
            id: i as u32 + 1,
            title: t.title.into(),
            artist_id: t.artist,
            file_path: format!("/{}", t.path),
            sample_rate: t.rate,
            sample_depth: t.depth,
            bitrate: 320,
            file_size: t.bytes.len() as u32,
            duration_secs: 360,
            tempo: 13_000,
        });
    }
    let n = fixture.len() as u32;
    let mut playlists = vec![
        FixturePlaylist { id: 1, parent_id: 0, name: "Gigs".into(), is_folder: true, sort_order: 0, track_ids: vec![] },
        FixturePlaylist {
            id: 2,
            parent_id: 1,
            name: "Berghain".into(),
            is_folder: false,
            sort_order: 0,
            track_ids: vec![5, 3, 7, 8],
        },
        FixturePlaylist {
            id: 3,
            parent_id: 0,
            name: "Warm-up".into(),
            is_folder: false,
            sort_order: 1,
            track_ids: vec![1, 2, 6],
        },
        FixturePlaylist {
            id: 4,
            parent_id: 0,
            name: "Peak time".into(),
            is_folder: false,
            sort_order: 2,
            track_ids: vec![3, 4, 5, 7, 8],
        },
    ];
    if hires {
        playlists.push(FixturePlaylist {
            id: 5,
            parent_id: 0,
            name: "Closing".into(),
            is_folder: false,
            sort_order: 3,
            track_ids: (9..=n).collect(),
        });
    }
    let artists = vec![
        (1, "Kölsch".to_string()),
        (2, "Charlotte de Witte".to_string()),
        (3, "Ben Klock".to_string()),
        (4, "Paula Temple".to_string()),
        (5, "Various".to_string()),
    ];
    write(root, DEVICE_LIBRARY_FILE, &build_pdb(&fixture, &artists, &playlists))?;
    write(root, "PIONEER/MYSETTING.DAT", &[0u8; 160])?;
    for t in &fixture {
        write(root, &format!("PIONEER/USBANLZ/P000/{:08X}/ANLZ0000.DAT", t.id), b"PMAI")?;
    }
    if litter {
        // Damaged download and macOS metadata litter.
        write(root, "Contents/Downloads/Unknown Track (1).mp3", b"<html>Access denied</html>")?;
        write(root, "Contents/Kolsch/Speicher/._Grey.mp3", &[0u8; 4096])?;
        write(root, "Contents/Hi-Res/._Ambient Intro.flac", &[0u8; 4096])?;
    }
    if one_library {
        // OneLibrary databases are encrypted; random-looking bytes stand in.
        let bytes: Vec<u8> = (0..65_536u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        write(root, ONE_LIBRARY_FILE, &bytes)?;
    }
    Ok(())
}

/// A simulated USB stick for demo mode.
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
    pub description: &'static str,
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
        description: "128 GB, formatted on a Mac (GPT + exFAT), rekordbox Device Library only",
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
        description: "32 GB, new and empty (GPT + exFAT)",
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
        description: "64 GB, MBR + FAT32 with both rekordbox libraries",
    },
];

fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// Create `<root>/<name>/{device.json, volume/}` in the demo platform's
/// folder format.
pub fn create_stick(root: &Path, s: &DemoStick) -> io::Result<PathBuf> {
    let dir = root.join(s.name);
    fs::create_dir_all(dir.join("volume"))?;
    let h = fnv(s.name);
    let mut dev = PhysicalDevice {
        id: format!("demo:{}", s.name),
        os_path: dir.join("disk.img").to_string_lossy().into_owned(),
        bus: BusType::Usb,
        removable: true,
        is_system: false,
        size_bytes: s.size,
        logical_sector_size: 512,
        storage_vendor: Some(s.maker.into()),
        storage_model: Some(s.product.into()),
        storage_revision: Some("1.00".into()),
        usb: Some(UsbDescriptor {
            vendor_id: Some(s.vid),
            product_id: Some(s.pid),
            manufacturer: Some(s.maker.into()),
            product: Some(s.product.into()),
            serial: Some(format!("DEMO{h:08X}")),
            bcd_device: Some(0x0100),
            usb_version: Some("3.20".into()),
            speed_mbps: Some(5000),
            max_power_ma: Some(224),
        }),
        partition_scheme: Some(s.scheme),
        volumes: Vec::new(),
    };
    // A GPT drive formatted on a Mac has a 200 MB EFI system partition in
    // front of the data partition, and the OS lists it as a FAT32 volume.
    let gpt = s.scheme == PartitionScheme::Gpt;
    if gpt {
        dev.volumes.push(Volume {
            os_path: format!("demo:{}:1", s.name),
            mount_point: None,
            label: Some("EFI".into()),
            filesystem: Some(FilesystemKind::Fat32),
            size_bytes: 209_715_200,
            offset_bytes: Some(20_480),
            uuid: None,
            efi_system: true,
        });
    }
    let offset: u64 = if gpt { 209_735_680 } else { 1 << 20 };
    dev.volumes.push(Volume {
        os_path: format!("demo:{}:{}", s.name, dev.volumes.len() + 1),
        mount_point: None,
        label: Some(s.label.into()),
        filesystem: Some(s.fs),
        size_bytes: s.size.saturating_sub(offset),
        offset_bytes: Some(offset),
        uuid: Some(format!("{:04X}-{:04X}", h >> 16, h & 0xFFFF)),
        efi_system: false,
    });
    fs::write(dir.join("device.json"), serde_json::to_vec_pretty(&dev).map_err(io::Error::other)?)?;
    populate(&dir.join("volume"), s.content)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{scan_libraries, LibraryFormat};
    use crate::scan::scan_audio;

    #[test]
    fn demo_contents_scan_as_intended() {
        let d = tempfile::tempdir().unwrap();
        populate(d.path(), DemoContent::DeviceLibraryWithIssues).unwrap();
        let libs = scan_libraries(d.path());
        assert_eq!(libs.formats, vec![LibraryFormat::RekordboxDeviceLibrary]);
        assert!(libs.problems().is_empty(), "{:?}", libs.problems());
        let audio = scan_audio(d.path(), |_, _| {});
        assert_eq!(audio.tracks.len(), 13);
        assert_eq!(audio.apple_double, 2);
        assert_eq!(
            audio.tracks.iter().filter(|t| matches!(t.probe, crate::scan::TrackProbe::Unreadable(_))).count(),
            1
        );

        let e = tempfile::tempdir().unwrap();
        populate(e.path(), DemoContent::BothLibraries).unwrap();
        let libs = scan_libraries(e.path());
        assert!(libs.has(LibraryFormat::RekordboxOneLibrary) && libs.has(LibraryFormat::RekordboxDeviceLibrary));
        assert!(libs.problems().is_empty(), "{:?}", libs.problems());
    }

    /// Without a raw read, the layout comes from the volumes the OS lists,
    /// and on a Mac GPT drive the first of those is the FAT32 EFI partition.
    /// Taking it as the data partition would judge an exFAT stick as FAT32.
    #[test]
    fn os_layout_skips_the_efi_partition() {
        let d = tempfile::tempdir().unwrap();
        let s = STICKS.iter().find(|s| s.scheme == PartitionScheme::Gpt).unwrap();
        let dir = create_stick(d.path(), s).unwrap();
        let dev: PhysicalDevice = serde_json::from_slice(&fs::read(dir.join("device.json")).unwrap()).unwrap();
        assert_eq!(dev.volumes[0].filesystem, Some(FilesystemKind::Fat32));
        let layout = crate::drive::layout_from_os(&dev);
        assert_eq!(layout.primary_filesystem(), Some(s.fs));
        assert!(!layout.primary_is_first());
        assert_eq!(dev.primary_volume().and_then(|v| v.label.as_deref()), Some(s.label));
    }
}
