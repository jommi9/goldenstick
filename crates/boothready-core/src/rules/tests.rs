use super::bundle::{sign_bundle, verify_bundle, BundleError, TrustedKey};
use super::*;
use crate::audio::{AudioInfo, Codec, Container};
use crate::library::LibraryFormat;
use boothready_model::{FilesystemKind, PartitionScheme};

fn rules() -> Ruleset {
    Ruleset::builtin()
}

fn info(codec: Codec, container: Container, rate: u32, bits: Option<u16>) -> AudioInfo {
    let mut i = AudioInfo::new(container, codec);
    i.sample_rate = Some(rate);
    i.bit_depth = bits;
    i.channels = Some(2);
    i
}

fn track(path: &str, i: AudioInfo) -> TrackFacts {
    TrackFacts { path: path.into(), info: Some(i), error: None }
}

fn facts(scheme: PartitionScheme, fs: FilesystemKind, libs: &[LibraryFormat], tracks: Vec<TrackFacts>) -> DriveFacts {
    DriveFacts {
        scheme,
        filesystem: Some(fs),
        fs_dirty: Some(false),
        primary_is_first: true,
        partition_count: 1,
        libraries: libs.to_vec(),
        damaged_libraries: vec![],
        device_library_missing: 0,
        device_library_changed: 0,
        engine_missing: 0,
        library_warnings: vec![],
        tracks,
        apple_double: 0,
        usb_max_power_ma: Some(200),
        usb_model: None,
    }
}

fn standard_tracks() -> Vec<TrackFacts> {
    vec![
        track("Contents/a.mp3", info(Codec::Mp3, Container::Mp3, 44_100, None)),
        track("Contents/b.aiff", info(Codec::Pcm, Container::Aiff, 44_100, Some(16))),
        track("Contents/c.wav", info(Codec::Pcm, Container::Wav, 48_000, Some(24))),
    ]
}

fn preset_devices<'a>(r: &'a Ruleset, id: &str) -> Vec<&'a DeviceProfile> {
    r.devices_by_id(&r.preset(id).unwrap().devices)
}

#[test]
fn builtin_ruleset_is_consistent() {
    let r = rules();
    assert!(r.devices.len() >= 15);
    for p in &r.presets {
        assert_eq!(r.devices_by_id(&p.devices).len(), p.devices.len(), "preset {}", p.id);
    }
    // Every device states something about MBR and FAT32, the layout we build.
    for d in &r.devices {
        assert!(d.partition_tables.contains_key(&PartitionScheme::Mbr), "{}", d.id);
        assert!(d.filesystems.contains_key(&FilesystemKind::Fat32), "{}", d.id);
        assert!(!d.audio.is_empty(), "{}", d.id);
    }
}

#[test]
fn claims_without_evidence_are_rejected() {
    assert!("supported/unknown".parse::<Claim>().is_err());
    assert!("maybe/vendor".parse::<Claim>().is_err());
    assert_eq!("unknown/unknown".parse::<Claim>().unwrap(), Claim::UNKNOWN);
    let bad = BUILTIN_RULESET.replacen("\"fat32\": \"supported/vendor\"", "\"fat32\": \"supported/unknown\"", 1);
    assert!(Ruleset::from_json(&bad).is_err());
}

#[test]
fn search_by_nickname() {
    let r = rules();
    let ids = |q: &str| r.search(q).iter().map(|d| d.id.clone()).collect::<Vec<_>>();
    let old_nexus = ids("old nexus");
    assert!(old_nexus[..2].contains(&"cdj-2000nxs".to_string()), "{old_nexus:?}");
    assert_eq!(ids("3000x")[0], "cdj-3000x");
    assert_eq!(ids("CDJ-2000")[0], "cdj-2000");
    assert_eq!(ids("nxs2")[0], "cdj-2000nxs2");
    assert!(ids("denon").iter().all(|id| id.starts_with("denon")));
    assert!(ids("zzzz nothing").is_empty());
}

#[test]
fn audio_rules() {
    let r = rules();
    let cdj2000 = r.device("cdj-2000").unwrap();
    let nxs2 = r.device("cdj-2000nxs2").unwrap();
    let x3000 = r.device("cdj-3000x").unwrap();
    let sc6000 = r.device("denon-sc6000").unwrap();

    let flac = info(Codec::Flac, Container::Flac, 44_100, Some(16));
    assert!(matches!(evaluate_audio(cdj2000, &flac), TrackVerdict::Unsupported { evidence: Evidence::Vendor, .. }));
    assert_eq!(evaluate_audio(nxs2, &flac), TrackVerdict::Supported { evidence: Evidence::Vendor });
    assert_eq!(evaluate_audio(x3000, &flac), TrackVerdict::Supported { evidence: Evidence::Inferred });

    let hires = info(Codec::Pcm, Container::Wav, 96_000, Some(24));
    match evaluate_audio(cdj2000, &hires) {
        TrackVerdict::Unsupported { reason, .. } => {
            assert!(reason.contains("96 kHz") && reason.contains("48 kHz"), "{reason}")
        }
        v => panic!("{v:?}"),
    }
    assert!(evaluate_audio(nxs2, &hires).is_supported());

    let float = info(Codec::PcmFloat, Container::Wav, 44_100, Some(32));
    assert!(!evaluate_audio(nxs2, &float).is_supported());
    let s32 = info(Codec::Pcm, Container::Wav, 44_100, Some(32));
    match evaluate_audio(nxs2, &s32) {
        TrackVerdict::Unsupported { reason, .. } => assert!(reason.contains("32-bit"), "{reason}"),
        v => panic!("{v:?}"),
    }

    let opus = info(Codec::Opus, Container::Ogg, 48_000, None);
    assert!(matches!(evaluate_audio(sc6000, &opus), TrackVerdict::Unknown { .. }));
    let mpeg2 = info(Codec::Mp3, Container::Mp3, 22_050, None);
    assert!(!evaluate_audio(cdj2000, &mpeg2).is_supported());
    let drm = info(Codec::Protected, Container::Mp4, 44_100, None);
    assert!(!evaluate_audio(r.device("mixxx").unwrap(), &drm).is_supported());
}

#[test]
fn format_recommendations() {
    let r = rules();
    let club = recommend_format(&preset_devices(&r, "unknown_club"));
    assert_eq!((club.scheme, club.filesystem), (PartitionScheme::Mbr, FilesystemKind::Fat32));
    assert!(club.why.contains("CDJ-2000"), "{}", club.why);
    assert!(club.why.contains("MBR"), "{}", club.why);
    assert!(club.not_covered.is_empty());

    let denon = recommend_format(&preset_devices(&r, "denon_engine"));
    assert_eq!(denon.filesystem, FilesystemKind::Exfat);
    assert!(denon.why.contains("Engine DJ recommends exFAT"));

    let mixxx = recommend_format(&preset_devices(&r, "laptop_mixxx"));
    assert_eq!(mixxx.filesystem, FilesystemKind::Fat32);
}

/// PRD §100: a 128 GB GPT/exFAT USB with only a Device Library, unknown club.
#[test]
fn prd_acceptance_scenario() {
    let r = rules();
    let targets = preset_devices(&r, "unknown_club");
    let f =
        facts(PartitionScheme::Gpt, FilesystemKind::Exfat, &[LibraryFormat::RekordboxDeviceLibrary], standard_tracks());
    let a = assess_drive(&f, &targets, &r);
    let dev = |id: &str| a.devices.iter().find(|d| d.device_id == id).unwrap();

    assert_eq!(a.overall, Verdict::FixNeeded);
    assert_eq!(dev("cdj-2000").verdict, Verdict::FixNeeded);
    assert_eq!(dev("cdj-3000").verdict, Verdict::AtRisk, "{:#?}", dev("cdj-3000").layers);
    let x = dev("cdj-3000x");
    assert_eq!(x.verdict, Verdict::FixNeeded);
    let lib = x.layers.iter().find(|l| l.layer == Layer::Library).unwrap();
    assert_eq!(lib.status, LayerStatus::Fail);
    assert!(lib.detail.contains("OneLibrary"), "{}", lib.detail);

    // Non-destructive fixes come first, the rebuild last.
    assert!(a.fixes.last().unwrap().destructive);
    assert!(matches!(
        a.fixes.last().unwrap().kind,
        FixKind::Reformat { scheme: PartitionScheme::Mbr, filesystem: FilesystemKind::Fat32 }
    ));
    let export = a.fixes.iter().find_map(|f| match &f.kind {
        FixKind::ExportRekordbox { formats } => Some(formats.clone()),
        _ => None,
    });
    let export = export.expect("export fix");
    assert!(
        export.contains(&LibraryFormat::RekordboxOneLibrary) && export.contains(&LibraryFormat::RekordboxDeviceLibrary)
    );
    assert_eq!(general_headline(&f, &r), "DJ players may not recognise this drive as formatted");
    let mbr_exfat = facts(PartitionScheme::Mbr, FilesystemKind::Exfat, &[], vec![]);
    assert_eq!(general_headline(&mbr_exfat, &r), "Fine for newer players, won't work on older ones");
}

#[test]
fn well_prepared_drive() {
    let r = rules();
    let targets = preset_devices(&r, "unknown_club");
    let mut tracks = standard_tracks();
    tracks.push(track("Contents/d.flac", info(Codec::Flac, Container::Flac, 44_100, Some(16))));
    let f = facts(
        PartitionScheme::Mbr,
        FilesystemKind::Fat32,
        &[LibraryFormat::RekordboxDeviceLibrary, LibraryFormat::RekordboxOneLibrary],
        tracks,
    );
    let a = assess_drive(&f, &targets, &r);
    let dev = |id: &str| a.devices.iter().find(|d| d.device_id == id).unwrap();
    assert_eq!(dev("cdj-3000").verdict, Verdict::Ready, "{:#?}", dev("cdj-3000").layers);
    assert_eq!(dev("cdj-2000nxs2").verdict, Verdict::Ready);
    // OneLibrary players: present but unreadable to us, and inferred specs.
    assert_eq!(dev("cdj-3000x").verdict, Verdict::ExpectedToWork);
    // FLAC won't play on the original CDJ-2000.
    let old = dev("cdj-2000");
    assert_eq!(old.verdict, Verdict::Partial);
    assert_eq!(old.audio.unsupported, 1);
    assert_eq!(a.tracks.scanned, 4);
    assert_eq!(a.tracks.compatible_everywhere, 3);
    assert_eq!(a.tracks.need_attention, 1);
    assert!(a.checks.iter().all(|c| c.status != LayerStatus::Fail), "{:?}", a.checks);
    assert!(!a.fixes.iter().any(|f| f.destructive));
    assert_eq!(general_headline(&f, &r), "Good general-purpose DJ USB");
}

#[test]
fn physical_and_filesystem_health() {
    let r = rules();
    let cdj2000 = r.device("cdj-2000").unwrap();
    let mut f =
        facts(PartitionScheme::Mbr, FilesystemKind::Fat32, &[LibraryFormat::RekordboxDeviceLibrary], standard_tracks());
    f.usb_max_power_ma = Some(896);
    f.fs_dirty = Some(true);
    let a = assess_drive(&f, &[cdj2000], &r);
    let d = &a.devices[0];
    assert_eq!(d.verdict, Verdict::AtRisk);
    assert_eq!(d.layers[0].status, LayerStatus::Warn);
    assert!(d.layers[0].detail.contains("896 mA"));
    assert!(a.fixes.iter().any(|f| f.kind == FixKind::RepairFilesystem));
    assert!(a.fixes.iter().any(|f| f.kind == FixKind::UseAnotherUsb));
}

#[test]
fn missing_tracks_and_empty_drives() {
    let r = rules();
    let cdj3000 = r.device("cdj-3000").unwrap();
    let mut f =
        facts(PartitionScheme::Mbr, FilesystemKind::Fat32, &[LibraryFormat::RekordboxDeviceLibrary], standard_tracks());
    f.device_library_missing = 12;
    let a = assess_drive(&f, &[cdj3000], &r);
    assert_eq!(a.devices[0].verdict, Verdict::Partial);
    assert!(a.fixes.iter().any(|f| f.kind == FixKind::ReexportRekordbox));

    let empty = facts(PartitionScheme::Mbr, FilesystemKind::Fat32, &[], vec![]);
    let a = assess_drive(&empty, &[cdj3000], &r);
    assert_eq!(a.headline, "Empty drive, ready to prepare");
    assert_eq!(a.devices[0].verdict, Verdict::Partial);

    // Loose files, no library: folder browsing only.
    let loose = facts(PartitionScheme::Mbr, FilesystemKind::Fat32, &[], standard_tracks());
    let a = assess_drive(&loose, &[cdj3000], &r);
    let lib = a.devices[0].layers.iter().find(|l| l.layer == Layer::Library).unwrap();
    assert_eq!(lib.headline, "Folder browsing only");
}

#[test]
fn engine_hardware_with_rekordbox_drive_is_not_assumed() {
    let r = rules();
    let sc6000 = r.device("denon-sc6000").unwrap();
    let f =
        facts(PartitionScheme::Mbr, FilesystemKind::Fat32, &[LibraryFormat::RekordboxDeviceLibrary], standard_tracks());
    let a = assess_drive(&f, &[sc6000], &r);
    assert_eq!(a.devices[0].verdict, Verdict::FixNeeded);
    assert!(a.fixes.iter().any(|f| f.kind == FixKind::ExportEngine));
    let lib = a.devices[0].layers.iter().find(|l| l.layer == Layer::Library).unwrap();
    assert!(lib.detail.contains("isn't documented"), "{}", lib.detail);
}

#[test]
fn signed_bundles() {
    let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let trusted = [TrustedKey { id: "release-1".into(), key: key.verifying_key() }];
    let newer = BUILTIN_RULESET.replacen("\"version\": 1,", "\"version\": 2,", 1);
    let bundle = serde_json::to_vec(&sign_bundle(&newer, "release-1", &key)).unwrap();
    let r = verify_bundle(&bundle, &trusted, 1).unwrap();
    assert_eq!(r.version, 2);

    assert!(matches!(verify_bundle(&bundle, &trusted, 2), Err(BundleError::Rollback { .. })));
    let other = sign_bundle(&newer, "someone", &ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]));
    assert!(matches!(
        verify_bundle(&serde_json::to_vec(&other).unwrap(), &trusted, 1),
        Err(BundleError::UnknownKey(_))
    ));
    let mut forged = sign_bundle(&newer, "release-1", &key);
    let tampered = newer.replacen("\"fat32\": \"supported/vendor\"", "\"fat32\": \"unsupported/vendor\"", 1);
    forged.payload = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, tampered.as_bytes());
    assert!(matches!(
        verify_bundle(&serde_json::to_vec(&forged).unwrap(), &trusted, 1),
        Err(BundleError::BadSignature)
    ));
    assert!(matches!(verify_bundle(b"{}", &trusted, 1), Err(BundleError::Format)));
}
