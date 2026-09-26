use super::*;
use boothready_core::privileged::DeviceFingerprint;

fn stick(root: &Path, name: &str, size: u64) -> PhysicalDevice {
    DemoPlatform::create_stick(root, name, "Kingston", "DataTraveler 3.0", 0x0951, 0x1666, size).unwrap();
    DemoPlatform::new(root.to_path_buf())
        .list_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.id == format!("demo:{name}"))
        .unwrap()
}

fn run(backend: &dyn Backend, req: HelperRequest) -> Vec<HelperEvent> {
    let mut events = Vec::new();
    handle(backend, req, &mut |e| events.push(e));
    events
}

fn prepare_req(dev: &PhysicalDevice, fs: FilesystemKind, label: &str) -> HelperRequest {
    let fp = DeviceFingerprint::of(dev);
    HelperRequest::Prepare {
        confirmation: fp.token(),
        expected: fp,
        scheme: PartitionScheme::Mbr,
        filesystem: fs,
        label: label.into(),
    }
}

#[test]
fn demo_prepare_fat32_end_to_end() {
    let d = tempfile::tempdir().unwrap();
    let dev = stick(d.path(), "legacy", 256 * 1024 * 1024);
    fs::write(d.path().join("legacy/volume/old.mp3"), b"old").unwrap();
    let backend = DemoBackend::new(d.path().to_path_buf());
    let events = run(&backend, prepare_req(&dev, FilesystemKind::Fat32, "br_legacy"));
    let steps: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            HelperEvent::Progress { step, .. } => Some(step.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(steps, vec!["checking", "unmounting", "partitioning", "formatting", "verifying"]);
    let HelperEvent::Done { detail } = events.last().unwrap() else { panic!("{events:?}") };
    let layout: MediaLayout = serde_json::from_str(detail).unwrap();
    assert_eq!(layout.primary_filesystem(), Some(FilesystemKind::Fat32));
    // The demo volume was reset and relabelled, like a real remount.
    assert!(!d.path().join("legacy/volume/old.mp3").exists());
    let after = backend.platform.list_devices().unwrap();
    assert_eq!(after[0].volumes[0].label.as_deref(), Some("BR_LEGACY"));
    assert!(after[0].volumes[0].mount_point.is_some());
}

#[test]
fn refuses_stale_or_forged_confirmation() {
    let d = tempfile::tempdir().unwrap();
    let dev = stick(d.path(), "main", 256 * 1024 * 1024);
    let backend = DemoBackend::new(d.path().to_path_buf());
    let fp = DeviceFingerprint::of(&dev);
    let forged = HelperRequest::Prepare {
        expected: fp.clone(),
        confirmation: "0".repeat(32),
        scheme: PartitionScheme::Mbr,
        filesystem: FilesystemKind::Fat32,
        label: "BR_MAIN".into(),
    };
    let events = run(&backend, forged);
    assert!(matches!(events.last(), Some(HelperEvent::Error { code: HelperErrorCode::Refused, .. })), "{events:?}");
    assert!(!d.path().join("main/disk.img").exists(), "nothing may be written on refusal");

    // The user swaps in a different stick at the same "port" after confirming.
    fs::remove_dir_all(d.path().join("main")).unwrap();
    DemoPlatform::create_stick(d.path(), "main", "SanDisk", "Ultra", 0x0781, 0x5581, 128 * 1024 * 1024).unwrap();
    let events = run(&backend, prepare_req(&dev, FilesystemKind::Fat32, "BR_MAIN"));
    assert!(matches!(events.last(), Some(HelperEvent::Error { code: HelperErrorCode::Refused, .. })), "{events:?}");
    assert!(!d.path().join("main/disk.img").exists());

    // And a drive that's gone entirely.
    fs::remove_dir_all(d.path().join("main")).unwrap();
    let events = run(&backend, prepare_req(&dev, FilesystemKind::Fat32, "BR_MAIN"));
    assert!(matches!(events.last(), Some(HelperEvent::Error { code: HelperErrorCode::NotFound, .. })), "{events:?}");
}

#[test]
fn rejects_bad_labels_and_layouts() {
    let d = tempfile::tempdir().unwrap();
    let dev = stick(d.path(), "x", 256 * 1024 * 1024);
    let backend = DemoBackend::new(d.path().to_path_buf());
    let events = run(&backend, prepare_req(&dev, FilesystemKind::Fat32, "rm -rf /"));
    assert!(matches!(events.last(), Some(HelperEvent::Error { code: HelperErrorCode::BadRequest, .. })));
    let events = run(&backend, prepare_req(&dev, FilesystemKind::Ntfs, "OK"));
    assert!(matches!(events.last(), Some(HelperEvent::Error { code: HelperErrorCode::Refused, .. })));
    let fp = DeviceFingerprint::of(&dev);
    let gpt = HelperRequest::Prepare {
        confirmation: fp.token(),
        expected: fp,
        scheme: PartitionScheme::Gpt,
        filesystem: FilesystemKind::Fat32,
        label: "OK".into(),
    };
    assert!(matches!(run(&backend, gpt).last(), Some(HelperEvent::Error { code: HelperErrorCode::Refused, .. })));
}

#[test]
fn busy_eject_names_the_holder() {
    let d = tempfile::tempdir().unwrap();
    stick(d.path(), "busy", 64 * 1024 * 1024);
    fs::write(d.path().join("busy/.busy"), "rekordbox").unwrap();
    let backend = DemoBackend::new(d.path().to_path_buf());
    match run(&backend, HelperRequest::Eject { device_id: "demo:busy".into() }).last() {
        Some(HelperEvent::Error { code: HelperErrorCode::Busy, message }) => {
            assert!(message.starts_with("rekordbox is using this USB"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn demo_prepare_exfat() {
    if !["/usr/sbin/mkfs.exfat", "/sbin/mkfs.exfat"].iter().any(|p| Path::new(p).exists()) {
        eprintln!("skipping: mkfs.exfat not installed");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let dev = stick(d.path(), "engine", 512 * 1024 * 1024);
    let backend = DemoBackend::new(d.path().to_path_buf());
    let events = run(&backend, prepare_req(&dev, FilesystemKind::Exfat, "BR_ENGINE"));
    let HelperEvent::Done { detail } = events.last().unwrap() else { panic!("{events:?}") };
    let layout: MediaLayout = serde_json::from_str(detail).unwrap();
    assert_eq!(layout.scheme, PartitionScheme::Mbr);
    assert_eq!(layout.primary_filesystem(), Some(FilesystemKind::Exfat));
    assert_eq!(layout.partitions[0].entry.mbr_type, Some(0x07));
}
