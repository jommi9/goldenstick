//! Building MBR + FAT32 media in pure Rust.
//!
//! Windows' own formatter refuses FAT32 above 32 GB, so BoothReady carries
//! its own path: a fixed MBR layout written here plus the `fatfs` crate's
//! formatter. Every image this module produces is checked in tests with
//! `fsck.fat`, `sfdisk` and mtools, and re-inspected with our own parser.
//!
//! This module only ever writes to the handle it is given. Choosing and
//! opening the right device is the privileged helper's job.

use crate::io::{Slice, MAX_SECTOR};
use boothready_model::{FilesystemKind, PartitionScheme};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek, SeekFrom, Write};

const MIB: u64 = 1024 * 1024;
/// MBR partition type for FAT32 with LBA addressing.
pub const MBR_TYPE_FAT32_LBA: u8 = 0x0C;
/// MBR partition type used for exFAT (and NTFS).
pub const MBR_TYPE_EXFAT: u8 = 0x07;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub scheme: PartitionScheme,
    pub filesystem: FilesystemKind,
    pub sector_size: u32,
    pub partition_start: u64,
    pub partition_len: u64,
    pub mbr_type: u8,
    pub cluster_size: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("device is too small ({0} bytes)")]
    TooSmall(u64),
    #[error("MBR can't address a device this large with {0}-byte sectors")]
    TooLargeForMbr(u32),
    #[error("{0} can't be created by BoothReady's built-in formatter")]
    Unsupported(String),
    #[error("volume label is invalid: {0}")]
    BadLabel(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Microsoft's default FAT32 cluster sizes, which DJ players are known to
/// handle because that is what Windows and macOS produce. Small volumes drop
/// to smaller clusters so the cluster count stays in FAT32 territory; with
/// too few clusters a volume is FAT16 no matter what its header says.
pub fn fat32_cluster_size(partition_bytes: u64) -> u32 {
    const GIB: u64 = 1024 * MIB;
    const MIN_CLUSTERS: u64 = 65_525 + 16;
    let mut size: u32 = match partition_bytes {
        b if b <= 8 * GIB => 4096,
        b if b <= 16 * GIB => 8192,
        b if b <= 32 * GIB => 16384,
        _ => 32768,
    };
    while size > 512 && partition_bytes / (size as u64) < MIN_CLUSTERS {
        size /= 2;
    }
    size
}

/// Validate and normalise a FAT volume label: at most 11 characters from
/// the set every player displays correctly.
pub fn normalize_label(label: &str) -> Result<String, FormatError> {
    let up = label.trim().to_ascii_uppercase();
    if up.is_empty() || up.len() > 11 {
        return Err(FormatError::BadLabel(format!("'{label}' must be 1 to 11 characters")));
    }
    if !up.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(FormatError::BadLabel(format!("'{label}' may only use A-Z, 0-9, '_' and '-'")));
    }
    Ok(up)
}

pub fn plan_layout(disk_bytes: u64, sector_size: u32, filesystem: FilesystemKind) -> Result<Layout, FormatError> {
    if !matches!(filesystem, FilesystemKind::Fat32 | FilesystemKind::Exfat) {
        return Err(FormatError::Unsupported(filesystem.label().into()));
    }
    let ss = sector_size as u64;
    let start = MIB;
    // Leave the last MiB alone: it's where a stale GPT backup header lived,
    // and we zero it so nothing mistakes the disk for GPT.
    let end = (disk_bytes.saturating_sub(MIB)) / ss * ss;
    if end <= start + 64 * MIB {
        return Err(FormatError::TooSmall(disk_bytes));
    }
    let len = end - start;
    if (start + len) / ss > u32::MAX as u64 {
        return Err(FormatError::TooLargeForMbr(sector_size));
    }
    if filesystem == FilesystemKind::Fat32 && len / ss > u32::MAX as u64 {
        return Err(FormatError::TooLargeForMbr(sector_size));
    }
    Ok(Layout {
        scheme: PartitionScheme::Mbr,
        filesystem,
        sector_size,
        partition_start: start,
        partition_len: len,
        mbr_type: if filesystem == FilesystemKind::Fat32 { MBR_TYPE_FAT32_LBA } else { MBR_TYPE_EXFAT },
        cluster_size: if filesystem == FilesystemKind::Fat32 { fat32_cluster_size(len) } else { 0 },
    })
}

fn zero_range<D: Write + Seek>(dev: &mut D, start: u64, len: u64) -> io::Result<()> {
    let buf = vec![0u8; MIB as usize];
    dev.seek(SeekFrom::Start(start))?;
    let mut left = len;
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        dev.write_all(&buf[..n])?;
        left -= n as u64;
    }
    Ok(())
}

/// Write a fresh MBR with a single partition. Also wipes the first and last
/// MiB so no GPT header or old boot sector survives.
pub fn write_mbr<D: Read + Write + Seek>(
    dev: &mut D,
    disk_bytes: u64,
    layout: &Layout,
    disk_signature: u32,
) -> io::Result<()> {
    let ss = layout.sector_size as u64;
    zero_range(dev, 0, MIB)?;
    let tail = disk_bytes / MAX_SECTOR * MAX_SECTOR;
    if tail > 2 * MIB {
        zero_range(dev, tail - MIB, MIB)?;
    }
    let mut mbr = vec![0u8; ss as usize];
    mbr[440..444].copy_from_slice(&disk_signature.to_le_bytes());
    let e = &mut mbr[446..462];
    e[0] = 0x00;
    // CHS fields beyond the 8 GB limit are conventionally set to the maximum.
    e[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    e[4] = layout.mbr_type;
    e[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    e[8..12].copy_from_slice(&((layout.partition_start / ss) as u32).to_le_bytes());
    e[12..16].copy_from_slice(&((layout.partition_len / ss) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    dev.seek(SeekFrom::Start(0))?;
    dev.write_all(&mbr)?;
    dev.flush()
}

/// Format the partition described by `layout` as FAT32.
pub fn format_fat32<D: Read + Write + Seek>(
    dev: &mut D,
    layout: &Layout,
    label: &str,
    volume_id: u32,
) -> Result<(), FormatError> {
    let label = normalize_label(label)?;
    let mut label_bytes = [b' '; 11];
    label_bytes[..label.len()].copy_from_slice(label.as_bytes());
    let ss = layout.sector_size as u64;
    {
        let part = Slice::new(&mut *dev, layout.partition_start, layout.partition_len);
        let opts = fatfs::FormatVolumeOptions::new()
            .fat_type(fatfs::FatType::Fat32)
            .bytes_per_sector(layout.sector_size as u16)
            .bytes_per_cluster(layout.cluster_size)
            .total_sectors((layout.partition_len / ss) as u32)
            .volume_id(volume_id)
            .volume_label(label_bytes);
        fatfs::format_volume(part, opts)?;
    }
    // fatfs leaves "hidden sectors" at 0 and the FSInfo free-cluster count
    // unset. Fill both in the way Windows and macOS do, in the primary
    // structures and their backups.
    let hidden = ((layout.partition_start / ss) as u32).to_le_bytes();
    let mut boot = vec![0u8; ss as usize];
    dev.seek(SeekFrom::Start(layout.partition_start))?;
    dev.read_exact(&mut boot)?;
    // Refuse to call it FAT32 unless it is: FAT16 layouts keep a 16-bit FAT
    // size and a fixed root directory.
    if u16::from_le_bytes([boot[22], boot[23]]) != 0 || u16::from_le_bytes([boot[17], boot[18]]) != 0 {
        return Err(FormatError::Io(io::Error::other("formatter produced FAT16 instead of FAT32")));
    }
    let reserved = u16::from_le_bytes([boot[14], boot[15]]) as u64;
    let fats = boot[16] as u64;
    let fat_size = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64;
    let spc = boot[13] as u64;
    let total = u32::from_le_bytes(boot[32..36].try_into().unwrap()) as u64;
    let clusters = (total - reserved - fats * fat_size) / spc;
    let fs_info = u16::from_le_bytes([boot[48], boot[49]]) as u64;
    let backup = u16::from_le_bytes([boot[50], boot[51]]) as u64;
    for sector in [0u64, backup] {
        let at = layout.partition_start + sector * ss;
        dev.seek(SeekFrom::Start(at))?;
        dev.read_exact(&mut boot)?;
        boot[28..32].copy_from_slice(&hidden);
        dev.seek(SeekFrom::Start(at))?;
        dev.write_all(&boot)?;
    }
    for sector in [fs_info, backup + fs_info] {
        let at = layout.partition_start + sector * ss;
        let mut info = vec![0u8; ss as usize];
        dev.seek(SeekFrom::Start(at))?;
        dev.read_exact(&mut info)?;
        if &info[0..4] == b"RRaA" && &info[484..488] == b"rrAa" {
            // The root directory occupies one cluster.
            info[488..492].copy_from_slice(&((clusters - 1) as u32).to_le_bytes());
            info[492..496].copy_from_slice(&3u32.to_le_bytes());
            dev.seek(SeekFrom::Start(at))?;
            dev.write_all(&info)?;
        }
    }
    dev.flush()?;
    Ok(())
}

/// Erase and build an MBR + FAT32 drive on `dev`, returning the layout used.
pub fn build_fat32<D: Read + Write + Seek>(
    dev: &mut D,
    disk_bytes: u64,
    sector_size: u32,
    label: &str,
    seed: u64,
) -> Result<Layout, FormatError> {
    let layout = plan_layout(disk_bytes, sector_size, FilesystemKind::Fat32)?;
    write_mbr(dev, disk_bytes, &layout, (seed >> 32) as u32 ^ seed as u32)?;
    format_fat32(dev, &layout, label, (seed as u32).rotate_left(13))?;
    Ok(layout)
}

#[cfg(test)]
mod tests;
