//! Building MBR + FAT32 media in pure Rust.
//!
//! Windows' own formatter refuses FAT32 above 32 GB, so BoothReady carries
//! its own: a fixed MBR layout and a FAT32 volume with the same geometry
//! mkfs.fat produces. Every image this module produces is checked in tests
//! with `fsck.fat`, `sfdisk` and mtools, compared with mkfs.fat's output,
//! and re-inspected with our own parser.
//!
//! This module only ever writes to the handle it is given. Choosing and
//! opening the right device is the privileged helper's job.

use crate::io::MAX_SECTOR;
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

/// Sectors reserved in front of the first FAT, as Windows and mkfs.fat do.
const FAT32_MIN_RESERVED: u32 = 32;
/// Sector of the backup boot sector; the backup FSInfo follows it.
const FAT32_BACKUP_BOOT: u32 = 6;
/// FAT entries at or above this mark bad or end-of-chain clusters.
const FAT32_MAX_CLUSTERS: u64 = 0x0FFF_FFF5;

/// Where everything goes in a FAT32 volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fat32Geometry {
    pub sectors_per_cluster: u32,
    pub reserved_sectors: u32,
    pub fat_sectors: u32,
    pub clusters: u32,
}

impl Fat32Geometry {
    /// First sector of the data region, relative to the partition.
    pub fn data_start(&self) -> u32 {
        self.reserved_sectors + 2 * self.fat_sectors
    }
}

/// Lay out a FAT32 volume the way mkfs.fat does: 32 reserved sectors, or
/// one cluster's worth when clusters are bigger, and FATs rounded up to
/// whole clusters, so both FATs and the data region start on cluster
/// boundaries. With the partition at 1 MiB, clusters then line up with the
/// flash pages underneath.
pub fn fat32_geometry(total_sectors: u32, sector_size: u32, cluster_size: u32) -> Result<Fat32Geometry, FormatError> {
    let spc = cluster_size / sector_size;
    if spc == 0 || !spc.is_power_of_two() || spc > 128 {
        return Err(FormatError::Unsupported(format!("FAT32 with {cluster_size}-byte clusters")));
    }
    let (total, spc64) = (total_sectors as u64, spc as u64);
    let entries_per_sector = sector_size as u64 / 4;
    let reserved = (FAT32_MIN_RESERVED as u64).max(spc64);
    // The smallest FAT that covers every cluster left after the reserved
    // area and both FATs, plus the two reserved FAT entries. Rounding it up
    // only leaves fewer clusters, so it stays big enough.
    let avail = total.saturating_sub(reserved);
    let fat = (avail + 2 * spc64).div_ceil(entries_per_sector * spc64 + 2).next_multiple_of(spc64);
    let data = reserved + 2 * fat;
    let clusters = total.saturating_sub(data) / spc64;
    if clusters < 65_525 {
        return Err(FormatError::TooSmall(total * sector_size as u64));
    }
    if clusters > FAT32_MAX_CLUSTERS || fat > u32::MAX as u64 {
        return Err(FormatError::Unsupported(format!("FAT32 with {clusters} clusters")));
    }
    debug_assert!(fat * entries_per_sector >= clusters + 2);
    Ok(Fat32Geometry {
        sectors_per_cluster: spc,
        reserved_sectors: reserved as u32,
        fat_sectors: fat as u32,
        clusters: clusters as u32,
    })
}

/// FAT's packed date and time. FAT stores local time; the formatter writes
/// UTC because the only timestamp it sets is the volume label's.
fn dos_date_time(unix: u64) -> (u16, u16) {
    let (days, secs) = ((unix / 86_400) as i64, unix % 86_400);
    // Howard Hinnant's days-to-civil conversion.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let date = ((year - 1980).clamp(0, 127) as u16) << 9 | (month as u16) << 5 | day as u16;
    let time = ((secs / 3600) as u16) << 11 | (((secs % 3600) / 60) as u16) << 5 | ((secs % 60) / 2) as u16;
    (date, time)
}

fn write_sector<D: Write + Seek>(dev: &mut D, at: u64, bytes: &[u8]) -> io::Result<()> {
    dev.seek(SeekFrom::Start(at))?;
    dev.write_all(bytes)
}

/// Format the partition described by `layout` as FAT32, following
/// Microsoft's FAT specification with the choices Windows and mkfs.fat make
/// where it leaves room: two FATs, the root directory in cluster 2, FSInfo
/// in sector 1, backups in sectors 6 and 7, and the label in both the boot
/// sector and the root directory. Every write covers whole sectors.
pub fn format_fat32<D: Read + Write + Seek>(
    dev: &mut D,
    layout: &Layout,
    label: &str,
    volume_id: u32,
    created_unix: u64,
) -> Result<(), FormatError> {
    let label = normalize_label(label)?;
    let mut label_bytes = [b' '; 11];
    label_bytes[..label.len()].copy_from_slice(label.as_bytes());
    let ss = layout.sector_size;
    let ssz = ss as usize;
    let total = u32::try_from(layout.partition_len / ss as u64).map_err(|_| FormatError::TooLargeForMbr(ss))?;
    let g = fat32_geometry(total, ss, layout.cluster_size)?;
    let base = layout.partition_start;
    let at = |sector: u32| base + sector as u64 * ss as u64;

    // Everything up to the end of the root directory's cluster starts out
    // zeroed, which also clears any old boot sector or FAT.
    zero_range(dev, base, (g.data_start() + g.sectors_per_cluster) as u64 * ss as u64)?;

    let mut boot = vec![0u8; ssz];
    boot[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    boot[3..11].copy_from_slice(b"MSWIN4.1");
    boot[11..13].copy_from_slice(&(ss as u16).to_le_bytes());
    boot[13] = g.sectors_per_cluster as u8;
    boot[14..16].copy_from_slice(&(g.reserved_sectors as u16).to_le_bytes());
    boot[16] = 2; // FATs
    boot[21] = 0xF8; // fixed media
    boot[24..26].copy_from_slice(&63u16.to_le_bytes()); // sectors per track
    boot[26..28].copy_from_slice(&255u16.to_le_bytes()); // heads
    boot[28..32].copy_from_slice(&((base / ss as u64) as u32).to_le_bytes()); // hidden sectors
    boot[32..36].copy_from_slice(&total.to_le_bytes());
    boot[36..40].copy_from_slice(&g.fat_sectors.to_le_bytes());
    boot[44..48].copy_from_slice(&2u32.to_le_bytes()); // root directory cluster
    boot[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
    boot[50..52].copy_from_slice(&(FAT32_BACKUP_BOOT as u16).to_le_bytes());
    boot[64] = 0x80; // drive number
    boot[66] = 0x29; // extended boot signature
    boot[67..71].copy_from_slice(&volume_id.to_le_bytes());
    boot[71..82].copy_from_slice(&label_bytes);
    boot[82..90].copy_from_slice(b"FAT32   ");
    // Not bootable: hand control back to the BIOS if anyone tries.
    boot[90..94].copy_from_slice(&[0xCD, 0x18, 0xEB, 0xFE]);
    boot[510] = 0x55;
    boot[511] = 0xAA;

    let mut info = vec![0u8; ssz];
    info[0..4].copy_from_slice(b"RRaA");
    info[484..488].copy_from_slice(b"rrAa");
    // The root directory already uses cluster 2.
    info[488..492].copy_from_slice(&(g.clusters - 1).to_le_bytes());
    info[492..496].copy_from_slice(&3u32.to_le_bytes());
    info[508..512].copy_from_slice(&[0x00, 0x00, 0x55, 0xAA]);

    for first in [0, FAT32_BACKUP_BOOT] {
        write_sector(dev, at(first), &boot)?;
        write_sector(dev, at(first + 1), &info)?;
    }

    // FAT entries 0 and 1 hold the media byte and the clean-shutdown bits;
    // entry 2 ends the root directory's one-cluster chain.
    let mut fat = vec![0u8; ssz];
    fat[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    fat[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    fat[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    for copy in 0..2 {
        write_sector(dev, at(g.reserved_sectors + copy * g.fat_sectors), &fat)?;
    }

    let (date, time) = dos_date_time(created_unix);
    let mut root = vec![0u8; ssz];
    root[0..11].copy_from_slice(&label_bytes);
    root[11] = 0x08; // volume label
    root[22..24].copy_from_slice(&time.to_le_bytes());
    root[24..26].copy_from_slice(&date.to_le_bytes());
    write_sector(dev, at(g.data_start()), &root)?;
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
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    write_mbr(dev, disk_bytes, &layout, (seed >> 32) as u32 ^ seed as u32)?;
    format_fat32(dev, &layout, label, (seed as u32).rotate_left(13), now)?;
    Ok(layout)
}

#[cfg(test)]
mod tests;
