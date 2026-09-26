use super::*;
use crate::io::AlignedIo;
use crate::media::inspect_path;
use crate::testutil::{have_tool, run_tool, sparse_file};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::Command;

const GIB: u64 = 1024 * MIB;

fn open_rw(p: &Path) -> File {
    OpenOptions::new().read(true).write(true).open(p).unwrap()
}

/// Copy the partition out of a disk image so fsck.fat can check it. Only the
/// metadata region holds data on a fresh volume, so copy that into a sparse
/// file of the full partition size instead of reading gigabytes of zeros.
fn extract_partition(img: &Path, layout: &Layout, out: &Path) {
    let meta = (layout.partition_len / 256).clamp(8 * MIB, 512 * MIB);
    let st = Command::new("dd")
        .arg(format!("if={}", img.display()))
        .arg(format!("of={}", out.display()))
        .args(["bs=1M", "conv=sparse", "status=none"])
        .arg(format!("skip={}", layout.partition_start / MIB))
        .arg(format!("count={}", meta / MIB))
        .status()
        .unwrap();
    assert!(st.success());
    File::options().write(true).open(out).unwrap().set_len(layout.partition_len).unwrap();
}

fn fsck(part: &Path) -> String {
    let out = Command::new(if Path::new("/usr/sbin/fsck.fat").exists() { "/usr/sbin/fsck.fat" } else { "fsck.fat" })
        .args(["-n", "-v"])
        .arg(part)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "fsck.fat failed:\n{text}");
    text
}

fn check_image(img: &Path, size: u64, label: &str) -> Layout {
    let layout = plan_layout(size, 512, FilesystemKind::Fat32).unwrap();
    let inspected = inspect_path(img).unwrap();
    assert_eq!(inspected.scheme, PartitionScheme::Mbr);
    assert!(inspected.warnings.is_empty(), "{:?}", inspected.warnings);
    assert_eq!(inspected.partitions.len(), 1);
    let p = &inspected.partitions[0];
    assert!(p.aligned_1mib);
    assert_eq!(p.entry.mbr_type, Some(MBR_TYPE_FAT32_LBA));
    let fs = p.filesystem.as_ref().unwrap();
    assert_eq!(fs.kind, FilesystemKind::Fat32);
    assert_eq!(fs.label.as_deref(), Some(label));
    assert_eq!(fs.dirty, Some(false));
    assert!(fs.warnings.is_empty(), "{:?}", fs.warnings);
    assert_eq!(fs.cluster_size, Some(fat32_cluster_size(layout.partition_len)));
    layout
}

#[test]
fn builds_valid_fat32_on_small_drive() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("usb.img");
    let size = 2 * GIB;
    sparse_file(&img, size);
    let layout = build_fat32(&mut open_rw(&img), size, 512, "br_legacy", 42).unwrap();
    assert_eq!(layout.cluster_size, 4096);
    check_image(&img, size, "BR_LEGACY");

    if have_tool("sfdisk") {
        let dump = run_tool("sfdisk", &["--dump", img.to_str().unwrap()], None);
        assert!(dump.contains("label: dos"), "{dump}");
        assert!(dump.contains("start=        2048"), "{dump}");
        assert!(dump.contains("type=c"), "{dump}");
    }
    if have_tool("fsck.fat") {
        let part = dir.path().join("part.img");
        extract_partition(&img, &layout, &part);
        let report = fsck(&part);
        assert!(report.contains("32 bit entries"), "{report}");
        assert!(report.contains("2048 hidden sectors"), "{report}");
        assert!(!report.contains("uninitialized"), "{report}");
    }
    if have_tool("mcopy") {
        // Write and read back through an independent FAT implementation.
        let src = dir.path().join("track.mp3");
        std::fs::write(&src, crate::audio::fixtures::mp3_cbr(40)).unwrap();
        let target = format!("{}@@1M", img.display());
        run_tool("mmd", &["-i", &target, "::Contents"], None);
        run_tool("mcopy", &["-i", &target, src.to_str().unwrap(), "::Contents/track.mp3"], None);
        let listing = run_tool("mdir", &["-i", &target, "::Contents"], None);
        assert!(listing.to_uppercase().contains("TRACK"), "{listing}");
        let label = run_tool("mlabel", &["-i", &target, "-s", "::"], None);
        assert!(label.contains("BR_LEGACY"), "{label}");
    }
}

#[test]
#[cfg_attr(windows, ignore = "NTFS files aren't sparse by default; a 64 GB image would really use 64 GB")]
fn builds_large_fat32_beyond_windows_limit() {
    // 64 GB: bigger than Windows' built-in FAT32 formatter allows.
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("big.img");
    let size = 64 * GIB;
    sparse_file(&img, size);
    let layout = build_fat32(&mut open_rw(&img), size, 512, "BR_MAIN", 7).unwrap();
    assert_eq!(layout.cluster_size, 32768);
    check_image(&img, size, "BR_MAIN");
    if have_tool("fsck.fat") {
        let part = dir.path().join("part.img");
        extract_partition(&img, &layout, &part);
        fsck(&part);
    }
}

#[test]
fn replaces_gpt_completely() {
    if !have_tool("sgdisk") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("gpt.img");
    let size = GIB;
    sparse_file(&img, size);
    run_tool("sgdisk", &["-n", "1:2048:0", "-t", "1:0700", img.to_str().unwrap()], None);
    assert_eq!(inspect_path(&img).unwrap().scheme, PartitionScheme::Gpt);
    build_fat32(&mut open_rw(&img), size, 512, "BR_MAIN", 1).unwrap();
    let layout = inspect_path(&img).unwrap();
    assert_eq!(layout.scheme, PartitionScheme::Mbr, "{:?}", layout.warnings);
    assert!(layout.warnings.is_empty(), "{:?}", layout.warnings);
    // sgdisk must no longer see a GPT (primary or backup).
    let out = Command::new("/usr/sbin/sgdisk").args(["-v", img.to_str().unwrap()]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("GPT: present"), "{text}");
}

#[test]
fn aligned_io_produces_identical_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.img");
    let b = dir.path().join("b.img");
    let size = 256 * MIB;
    sparse_file(&a, size);
    sparse_file(&b, size);
    build_fat32(&mut open_rw(&a), size, 512, "SAME", 99).unwrap();
    let mut aligned = AlignedIo::new(open_rw(&b), 4096, size);
    build_fat32(&mut aligned, size, 512, "SAME", 99).unwrap();
    aligned.into_inner().unwrap();
    let (x, y) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert!(x == y, "raw and aligned formatting differ");
}

#[test]
fn rejects_bad_input() {
    assert!(matches!(plan_layout(32 * MIB, 512, FilesystemKind::Fat32), Err(FormatError::TooSmall(_))));
    assert!(matches!(plan_layout(3 * 1024 * GIB, 512, FilesystemKind::Fat32), Err(FormatError::TooLargeForMbr(512))));
    assert!(matches!(plan_layout(8 * GIB, 512, FilesystemKind::Ntfs), Err(FormatError::Unsupported(_))));
    // Small volumes get smaller clusters so they still count as FAT32.
    for mib in [100u64, 256, 300, 1024] {
        let l = plan_layout(mib * MIB, 512, FilesystemKind::Fat32).unwrap();
        assert!(l.partition_len / l.cluster_size as u64 >= 65_525 + 16, "{mib} MiB");
    }
    assert!(normalize_label("festival 2026").is_err());
    assert!(normalize_label("").is_err());
    assert!(normalize_label("TWELVECHARSX").is_err());
    assert_eq!(normalize_label(" br_main ").unwrap(), "BR_MAIN");
    let exfat = plan_layout(64 * GIB, 512, FilesystemKind::Exfat).unwrap();
    assert_eq!(exfat.mbr_type, MBR_TYPE_EXFAT);
}
