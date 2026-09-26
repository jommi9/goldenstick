use super::*;
use crate::library::scan_libraries;
use crate::manifest::{write_manifest, VerifyMode};
use crate::verify::{verify_volume, Expectations, VerifyRequest};

const EXFAT: CopyTarget =
    CopyTarget { filesystem: Some(FilesystemKind::Exfat), free_bytes: 1 << 40, cluster_bytes: 128 * 1024 };

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn full_verify(root: &Path) -> crate::verify::VerifyReport {
    let libs = scan_libraries(root);
    let manifest = read_manifest(root);
    let req = VerifyRequest {
        root,
        layout: None,
        expect: Expectations { scheme: None, filesystem: None },
        libs: &libs,
        manifest: manifest.as_ref(),
        mode: VerifyMode::Full,
    };
    verify_volume(&req, &AtomicBool::new(false), |_| {})
}

/// A copy of a real rekordbox export is a working export: the library
/// resolves, analysis files come along, every file verifies against the
/// hashes taken while copying, and a flipped byte is caught.
#[cfg(feature = "fixtures")]
#[test]
fn copied_export_is_complete_and_verifiable() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    crate::demo::populate(src.path(), crate::demo::DemoContent::BothLibraries).unwrap();
    write(src.path(), "Contents/._litter.mp3", b"AppleDouble");
    write(src.path(), ".Spotlight-V100/store.db", b"index");

    let plan = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    assert!(plan.can_start(), "{:?}", plan.problems);
    assert!(plan.files.iter().any(|f| f.path.contains("/USBANLZ/")), "analysis files must be copied");
    assert!(!plan.files.iter().any(|f| f.path.contains("._") || f.path.contains("Spotlight")));
    let mut ticks = 0;
    let report = run_copy(&plan, src.path(), dst.path(), &AtomicBool::new(false), |_| ticks += 1).unwrap();
    assert_eq!(report.files_copied, plan.files_to_copy);
    assert!(ticks > 0 && !report.cancelled);

    let (a, b) = (scan_libraries(src.path()), scan_libraries(dst.path()));
    assert_eq!(a.formats, b.formats);
    assert!(b.problems().is_empty(), "{:?}", b.problems());
    for f in &plan.files {
        let m = fs::metadata(dst.path().join(&f.path)).unwrap();
        assert_eq!(m.len(), f.size, "{}", f.path);
    }
    let manifest = read_manifest(dst.path()).unwrap();
    assert_eq!(manifest.files.len(), plan.files.len());
    let r = full_verify(dst.path());
    assert!(r.passed, "{:?} {:?}", r.checks, r.failures);

    let track = plan.files.iter().find(|f| f.path.ends_with(".mp3") && f.size > 4096).unwrap();
    let mut bytes = fs::read(dst.path().join(&track.path)).unwrap();
    bytes[4000] ^= 0xFF;
    fs::write(dst.path().join(&track.path), bytes).unwrap();
    let r = full_verify(dst.path());
    assert!(r.failures.iter().any(|f| f.path == track.path && f.problem.contains("differ")), "{:?}", r.failures);
}

#[test]
fn copying_an_unchanged_drive_again_copies_nothing() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write(src.path(), "Contents/a.mp3", &[1; 5000]);
    write(src.path(), "Contents/b.mp3", &[2; 7000]);
    let plan = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    run_copy(&plan, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap();

    let again = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    assert!(again.can_start(), "{:?}", again.problems);
    assert_eq!((again.files_to_copy, again.files_done, again.bytes_to_copy), (0, 2, 0));
    // A verification recorded after the first copy survives a copy that
    // changes nothing.
    let mut m = read_manifest(dst.path()).unwrap();
    m.verification = Some(full_verify(dst.path()).record());
    write_manifest(dst.path(), &m).unwrap();
    let r = run_copy(&again, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!((r.files_copied, r.files_skipped), (0, 2));
    assert!(read_manifest(dst.path()).unwrap().verification.is_some());

    // A changed source file is copied again; the other is left alone.
    std::thread::sleep(Duration::from_millis(1100));
    write(src.path(), "Contents/b.mp3", &[3; 7000]);
    let changed = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    assert_eq!((changed.files_to_copy, changed.files_done), (1, 1));
    run_copy(&changed, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(fs::read(dst.path().join("Contents/b.mp3")).unwrap(), vec![3; 7000]);
    assert!(read_manifest(dst.path()).unwrap().verification.is_none(), "a changed copy needs verifying again");
}

#[test]
fn an_interrupted_copy_resumes_where_it_stopped() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for i in 0..6 {
        // About 3 MB each, so each file spans several read buffers.
        write(src.path(), &format!("Contents/{i}.mp3"), &crate::audio::fixtures::mp3_cbr(7_500 + i));
    }
    let plan = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    let cancel = AtomicBool::new(false);
    // Cancel as the third file starts.
    let r = run_copy(&plan, src.path(), dst.path(), &cancel, |p| {
        if p.files_done == 2 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(r.cancelled);
    assert_eq!(r.files_copied, 2);
    // No half-written file is left behind, and nothing counts as verified yet.
    assert!(list_files(dst.path()).unwrap().iter().all(|f| !f.path.ends_with(PART_SUFFIX)));
    assert!(read_manifest(dst.path()).is_none());

    let resumed = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    assert_eq!((resumed.files_done, resumed.files_to_copy), (2, 4));
    let r = run_copy(&resumed, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!((r.files_copied, r.files_skipped, r.cancelled), (4, 2, false));
    assert_eq!(read_manifest(dst.path()).unwrap().files.len(), 6);
    let v = full_verify(dst.path());
    assert!(v.passed, "{:?}", v.failures);
}

#[test]
fn files_removed_from_the_source_are_removed_from_the_copy() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write(src.path(), "Contents/keep.mp3", &[1; 100]);
    write(src.path(), "Contents/gone.mp3", &[2; 100]);
    let plan = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    run_copy(&plan, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap();
    fs::remove_file(src.path().join("Contents/gone.mp3")).unwrap();

    let plan = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    assert!(plan.can_start(), "{:?}", plan.problems);
    assert_eq!(plan.stale, vec!["Contents/gone.mp3".to_string()]);
    let r = run_copy(&plan, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(r.files_removed, 1);
    assert!(!dst.path().join("Contents/gone.mp3").exists());
    assert_eq!(read_manifest(dst.path()).unwrap().files.len(), 1);
}

#[test]
fn someone_elses_files_block_the_copy() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write(src.path(), "Contents/a.mp3", &[1; 100]);
    write(dst.path(), "Music/other.mp3", &[9; 100]);
    // OS litter and our own folder don't count.
    write(dst.path(), "System Volume Information/IndexerVolumeGuid", b"x");
    write(dst.path(), ".boothready/manifest.json", b"{}");
    let plan = plan_copy(src.path(), dst.path(), &EXFAT).unwrap();
    assert_eq!(plan.problems, vec![CopyProblem::DestinationNotEmpty { files: 1, example: "Music/other.mp3".into() }]);
    let err = run_copy(&plan, src.path(), dst.path(), &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(matches!(err, CopyError::Blocked(_)));
    assert!(dst.path().join("Music/other.mp3").exists());
    assert!(!dst.path().join("Contents/a.mp3").exists());
}

#[test]
fn space_is_checked_in_whole_clusters() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for i in 0..100 {
        write(src.path(), &format!("Contents/{i}.mp3"), &[0; 10]);
    }
    // 100 tiny files still take 100 clusters.
    let tight = CopyTarget { free_bytes: SPACE_MARGIN + 100 * 1024 * 1024, cluster_bytes: 1024 * 1024, ..EXFAT };
    let plan = plan_copy(src.path(), dst.path(), &tight).unwrap();
    assert!(matches!(plan.problems.as_slice(), [CopyProblem::NotEnoughSpace { .. }]), "{:?}", plan.problems);
    let roomy = CopyTarget { free_bytes: SPACE_MARGIN + 102 * 1024 * 1024, ..tight };
    assert!(plan_copy(src.path(), dst.path(), &roomy).unwrap().can_start());
}

#[test]
#[cfg_attr(windows, ignore = "NTFS files aren't sparse by default")]
fn fat32_refuses_files_of_4_gb_and_more() {
    let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write(src.path(), "Contents/set.wav", b"");
    File::options().write(true).open(src.path().join("Contents/set.wav")).unwrap().set_len(FAT32_MAX_FILE + 1).unwrap();
    let fat32 = CopyTarget { filesystem: Some(FilesystemKind::Fat32), ..EXFAT };
    let plan = plan_copy(src.path(), dst.path(), &fat32).unwrap();
    assert_eq!(
        plan.problems,
        vec![CopyProblem::TooLargeForFat32 { path: "Contents/set.wav".into(), size: FAT32_MAX_FILE + 1 }]
    );
    assert!(plan_copy(src.path(), dst.path(), &EXFAT).unwrap().can_start());
}

#[test]
fn names_fat_and_windows_refuse() {
    assert_eq!(name_problem("Contents/Artist/Track (Extended Mix).mp3"), None);
    assert_eq!(name_problem("Contents/Björk/Jóga.flac"), None);
    assert!(name_problem("Contents/What?.mp3").unwrap().contains("'?'"));
    assert!(name_problem("Contents/a:b.mp3").is_some());
    assert!(name_problem("Contents/dir./a.mp3").is_some());
    assert!(name_problem("Contents/CON.mp3").unwrap().contains("reserved"));
    assert!(name_problem(&format!("Contents/{}.mp3", "x".repeat(260))).is_some());
}

#[test]
fn a_drive_is_never_copied_onto_itself() {
    let src = tempfile::tempdir().unwrap();
    write(src.path(), "Contents/a.mp3", &[1; 10]);
    let err = plan_copy(src.path(), src.path(), &EXFAT).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    fs::create_dir_all(src.path().join("inside")).unwrap();
    assert!(plan_copy(src.path(), &src.path().join("inside"), &EXFAT).is_err());
}
