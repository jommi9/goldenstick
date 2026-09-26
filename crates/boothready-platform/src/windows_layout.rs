//! Parser for the `DRIVE_LAYOUT_INFORMATION_EX` buffer that Windows returns
//! from `IOCTL_DISK_GET_DRIVE_LAYOUT_EX`. It's plain byte parsing so the
//! tests run on every OS; `windows.rs` checks the offsets against the
//! `windows` crate's struct definitions at compile time.

#![cfg_attr(not(windows), allow(dead_code))]

use boothready_model::PartitionScheme;

/// Byte offset of `PartitionEntry` in `DRIVE_LAYOUT_INFORMATION_EX`.
pub(crate) const LAYOUT_ENTRIES: usize = 48;
/// Size of one `PARTITION_INFORMATION_EX`.
pub(crate) const ENTRY_SIZE: usize = 144;
/// Offset of `StartingOffset` in `PARTITION_INFORMATION_EX`.
pub(crate) const ENTRY_START: usize = 8;
/// Offset of the MBR/GPT union, which starts with the partition type.
pub(crate) const ENTRY_TYPE: usize = 32;

/// The EFI system partition type GUID in its in-memory (and on-disk) order.
const EFI_SYSTEM_GUID: [u8; 16] =
    [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DriveLayout {
    pub scheme: PartitionScheme,
    /// Starting byte offsets of EFI system partitions, which match the
    /// offsets that volume disk extents report.
    pub efi_starts: Vec<u64>,
}

pub(crate) fn parse(buf: &[u8]) -> Option<DriveLayout> {
    let style = u32::from_le_bytes(buf.get(0..4)?.try_into().ok()?);
    let count = u32::from_le_bytes(buf.get(4..8)?.try_into().ok()?) as usize;
    let scheme = match style {
        0 => PartitionScheme::Mbr,
        1 => PartitionScheme::Gpt,
        _ => PartitionScheme::Unknown,
    };
    let mut efi_starts = Vec::new();
    for i in 0..count.min(256) {
        let at = LAYOUT_ENTRIES + i * ENTRY_SIZE;
        let Some(e) = buf.get(at..at + ENTRY_SIZE) else { break };
        let efi = match scheme {
            PartitionScheme::Mbr => e[ENTRY_TYPE] == 0xEF,
            PartitionScheme::Gpt => e[ENTRY_TYPE..ENTRY_TYPE + 16] == EFI_SYSTEM_GUID,
            _ => false,
        };
        if efi {
            efi_starts.push(u64::from_le_bytes(e[ENTRY_START..ENTRY_START + 8].try_into().unwrap()));
        }
    }
    Some(DriveLayout { scheme, efi_starts })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(style: u32, entries: &[(u64, &[u8])]) -> Vec<u8> {
        let mut buf = vec![0u8; LAYOUT_ENTRIES + entries.len() * ENTRY_SIZE];
        buf[0..4].copy_from_slice(&style.to_le_bytes());
        buf[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for (i, (start, ty)) in entries.iter().enumerate() {
            let at = LAYOUT_ENTRIES + i * ENTRY_SIZE;
            buf[at..at + 4].copy_from_slice(&style.to_le_bytes());
            buf[at + ENTRY_START..at + ENTRY_START + 8].copy_from_slice(&start.to_le_bytes());
            buf[at + ENTRY_TYPE..at + ENTRY_TYPE + ty.len()].copy_from_slice(ty);
        }
        buf
    }

    // Microsoft basic data, EBD0A0A2-B9E5-4433-87C0-68B6B72699C7.
    const BASIC_DATA: [u8; 16] =
        [0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7];

    #[test]
    fn finds_the_efi_partition_on_a_mac_formatted_gpt_drive() {
        let buf = layout(1, &[(20_480, &EFI_SYSTEM_GUID), (209_735_680, &BASIC_DATA)]);
        assert_eq!(parse(&buf), Some(DriveLayout { scheme: PartitionScheme::Gpt, efi_starts: vec![20_480] }));
    }

    #[test]
    fn mbr_type_ef_is_efi_and_other_types_are_not() {
        let buf = layout(0, &[(1 << 20, &[0x0C]), (1 << 30, &[0xEF]), (0, &[0])]);
        assert_eq!(parse(&buf), Some(DriveLayout { scheme: PartitionScheme::Mbr, efi_starts: vec![1 << 30] }));
    }

    #[test]
    fn short_or_lying_buffers_do_not_panic() {
        let mut buf = layout(1, &[(20_480, &EFI_SYSTEM_GUID)]);
        buf[4..8].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(parse(&buf).unwrap().efi_starts, vec![20_480]);
        assert_eq!(parse(&buf[..LAYOUT_ENTRIES + 10]).unwrap().efi_starts, Vec::<u64>::new());
        assert_eq!(parse(&buf[..6]), None);
    }
}
