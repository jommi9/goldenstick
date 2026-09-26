//! Non-destructive structural inspection of a disk or disk image.

pub mod filesystem;
pub mod partition;

use crate::io::Disk;
use boothready_model::{FilesystemKind, PartitionScheme};
use filesystem::{detect_filesystem, FilesystemInfo};
use partition::{read_partition_map, PartitionEntry, PartitionKind, PartitionMap};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectedPartition {
    #[serde(flatten)]
    pub entry: PartitionEntry,
    pub filesystem: Option<FilesystemInfo>,
    /// Starts on a 1 MiB boundary, which flash controllers and every modern
    /// formatter expect.
    pub aligned_1mib: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaLayout {
    pub size_bytes: u64,
    pub scheme: PartitionScheme,
    pub sector_size: u32,
    pub disk_id: Option<String>,
    pub partitions: Vec<InspectedPartition>,
    pub warnings: Vec<String>,
}

impl MediaLayout {
    /// The partition a DJ player will mount: the first one holding a data
    /// filesystem, skipping EFI system partitions that macOS creates on GPT.
    pub fn primary(&self) -> Option<&InspectedPartition> {
        self.partitions
            .iter()
            .filter(|p| p.entry.kind != PartitionKind::EfiSystem)
            .find(|p| p.filesystem.is_some())
            .or_else(|| self.partitions.first())
    }

    pub fn primary_filesystem(&self) -> Option<FilesystemKind> {
        self.primary().and_then(|p| p.filesystem.as_ref()).map(|f| f.kind)
    }

    /// Players read the first partition entry. A primary data partition that
    /// is not first (macOS GPT layout puts EFI first) is worth flagging.
    pub fn primary_is_first(&self) -> bool {
        match (self.primary(), self.partitions.first()) {
            (Some(p), Some(f)) => p.entry.index == f.entry.index,
            _ => false,
        }
    }
}

/// Inspect any seekable disk or image.
pub fn inspect<R: Read + Seek>(reader: R) -> io::Result<MediaLayout> {
    let mut disk = Disk::from_seekable(reader)?;
    inspect_disk(&mut disk)
}

/// Inspect a disk image or (with sufficient privileges) a raw device node.
pub fn inspect_path(path: &Path) -> io::Result<MediaLayout> {
    inspect(File::open(path)?)
}

pub fn inspect_disk<R: Read + Seek>(disk: &mut Disk<R>) -> io::Result<MediaLayout> {
    let size = disk.size();
    let PartitionMap { scheme, sector_size, partitions, mut warnings, disk_id } = read_partition_map(disk)?;
    let mut inspected = Vec::new();
    for entry in partitions {
        let fs = if entry.kind == PartitionKind::Extended {
            None
        } else {
            detect_filesystem(disk, entry.start_bytes, entry.size_bytes.min(size.saturating_sub(entry.start_bytes)))?
        };
        if let Some(f) = &fs {
            if let Some(w) = type_mismatch(&entry, f.kind) {
                warnings.push(w);
            }
        }
        inspected.push(InspectedPartition {
            aligned_1mib: entry.start_bytes % (1024 * 1024) == 0,
            entry,
            filesystem: fs,
        });
    }
    let mut scheme = scheme;
    // HFS+ and APFS can live at sector 0 with no 0x55AA signature.
    if scheme == PartitionScheme::Unknown {
        if let Some(fs) = detect_filesystem(disk, 0, size)? {
            scheme = PartitionScheme::Superfloppy;
            warnings.retain(|w| w != "No recognisable partition table");
            warnings.push("Filesystem starts at sector 0 without a partition table".into());
            inspected.push(InspectedPartition {
                entry: PartitionEntry {
                    index: 1,
                    start_bytes: 0,
                    size_bytes: size,
                    kind: PartitionKind::Other,
                    mbr_type: None,
                    gpt_type: None,
                    gpt_name: None,
                    bootable: false,
                },
                filesystem: Some(fs),
                aligned_1mib: true,
            });
        }
    }
    Ok(MediaLayout { size_bytes: size, scheme, sector_size, disk_id, partitions: inspected, warnings })
}

/// Some players pick a driver from the MBR type byte rather than probing,
/// so a FAT32 volume behind an NTFS/exFAT type (or vice versa) matters.
fn type_mismatch(entry: &PartitionEntry, fs: FilesystemKind) -> Option<String> {
    let t = entry.mbr_type?;
    let ok = match fs {
        FilesystemKind::Fat32 => matches!(t, 0x0B | 0x0C),
        FilesystemKind::Fat16 => matches!(t, 0x04 | 0x06 | 0x0E),
        FilesystemKind::Fat12 => t == 0x01,
        FilesystemKind::Exfat | FilesystemKind::Ntfs => t == 0x07,
        FilesystemKind::HfsPlus => t == 0xAF,
        _ => true,
    };
    (!ok).then(|| format!("Partition {} is marked as type 0x{t:02X} but contains {}", entry.index, fs.label()))
}

#[cfg(test)]
mod tests;
