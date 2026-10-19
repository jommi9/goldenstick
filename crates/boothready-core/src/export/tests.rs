use super::device_library::*;
use crate::library::{scan_libraries, LibraryFormat};
use djbooth_analyzer::{AnlzFile, DatAnalysis, ExtAnalysis, ThreeBandAnalysis};
use std::fs;
use std::path::{Path, PathBuf};

fn click_track(path: &Path, bpm: f64, seconds: u32) {
    fs::write(path, crate::audio::fixtures::click_track_wav(bpm, seconds)).unwrap();
}

fn tag(path: &Path, title: &str, artist: &str) {
    use lofty::config::WriteOptions;
    use lofty::prelude::{Accessor, TagExt};
    use lofty::tag::{Tag, TagType};
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(title.into());
    tag.set_artist(artist.into());
    tag.set_album("Test Album".into());
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

fn source_tracks(dir: &Path, names: &[(&str, &str, &str)], seconds: u32) -> Vec<PathBuf> {
    names
        .iter()
        .enumerate()
        .map(|(i, (file, title, artist))| {
            let path = dir.join(file);
            click_track(&path, 120.0 + i as f64 * 4.0, seconds);
            tag(&path, title, artist);
            path
        })
        .collect()
}

#[test]
fn export_without_analysis_reads_back_through_the_library_scanner() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(
        &src,
        &[
            ("one.wav", "First Track", "Ääkkönen"),
            ("two - the sequel.wav", "Second", "Artist B"),
            ("three.wav", "Third", "Ääkkönen"),
        ],
        2,
    );
    let root = tmp.path().join("stick");
    let request = ExportRequest {
        root: root.clone(),
        tracks: tracks.clone(),
        playlists: vec![
            PlaylistSpec { name: "Friday".into(), tracks: vec![2, 0] },
            PlaylistSpec { name: "Warm-up ☀".into(), tracks: vec![1] },
        ],
        analysis: Analysis::Skip,
        analysis_cache: None,
    };
    let mut phases = Vec::new();
    let report = export_device_library(&request, |p| phases.push(p.phase)).unwrap();
    assert!(phases.contains(&"probe") && !phases.contains(&"analyze"), "{phases:?}");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.tracks.len(), 3);
    assert_eq!(report.playlists, 2);
    assert_eq!(report.analysis_files, 0);
    assert_eq!(report.tracks[1].usb_path, "/Contents/00000002/two - the sequel.wav");
    assert_eq!(report.tracks[0].title, "First Track");
    assert_eq!(report.tracks[0].artist.as_deref(), Some("Ääkkönen"));
    assert!(phases.contains(&"copy") && phases.contains(&"install"));
    assert!(root.join("Contents/00000001/one.wav").is_file());
    assert!(root.join("PIONEER/rekordbox/export.pdb").is_file());
    assert!(!root.join("PIONEER/USBANLZ").exists());
    let leftovers: Vec<_> =
        fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers.iter().filter(|n| n.starts_with(".boothready-export")).count(), 0, "{leftovers:?}");

    let libs = scan_libraries(&root);
    assert!(libs.has(LibraryFormat::RekordboxDeviceLibrary));
    let dl = libs.rekordbox.as_ref().unwrap().device_library.as_ref().unwrap();
    assert_eq!(dl.parse_error, None);
    assert!(dl.warnings.is_empty(), "{:?}", dl.warnings);
    assert_eq!(dl.tracks.len(), 3);
    assert!(dl.missing.is_empty() && dl.size_mismatch.is_empty(), "{:?} {:?}", dl.missing, dl.size_mismatch);
    let titles: Vec<(&str, Option<&str>, &str)> =
        dl.tracks.iter().map(|t| (t.title.as_str(), t.artist.as_deref(), t.path.as_str())).collect();
    assert_eq!(
        titles,
        vec![
            ("First Track", Some("Ääkkönen"), "Contents/00000001/one.wav"),
            ("Second", Some("Artist B"), "Contents/00000002/two - the sequel.wav"),
            ("Third", Some("Ääkkönen"), "Contents/00000003/three.wav"),
        ]
    );
    assert_eq!(dl.tracks[0].db_sample_rate, Some(44_100));
    assert_eq!(dl.tracks[0].db_bit_depth, Some(16));
    assert_eq!(dl.tracks[0].db_duration_secs, Some(2));
    let playlists: Vec<(&str, &[u32])> =
        dl.playlists.iter().map(|p| (p.name.as_str(), p.track_ids.as_slice())).collect();
    assert_eq!(playlists, vec![("Friday", &[3u32, 1][..]), ("Warm-up ☀", &[2u32][..])]);
    assert_eq!(libs.rekordbox.as_ref().unwrap().analysis_files, 0);
    assert!(libs.problems().is_empty(), "{:?}", libs.problems());
}

#[test]
fn export_with_analysis_points_the_anlz_files_at_the_media_path() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(&src, &[("clicks.wav", "Clicks", "Metronome")], 20);
    let root = tmp.path().join("stick");
    let request = ExportRequest {
        root: root.clone(),
        tracks,
        playlists: vec![PlaylistSpec { name: "Set".into(), tracks: vec![0] }],
        analysis: Analysis::Normal,
        analysis_cache: None,
    };
    let report = export_device_library(&request, |_| {}).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.analysis_files, 3);
    let track = &report.tracks[0];
    assert_eq!(track.analysis_files, 3);
    let bpm = track.tempo_bpm.unwrap();
    assert!((bpm - 120.0).abs() < 0.6, "tempo {bpm}");
    assert!(track.key.is_some());
    assert_eq!(track.duration_secs, 20);

    let libs = scan_libraries(&root);
    let rb = libs.rekordbox.as_ref().unwrap();
    assert_eq!(rb.analysis_files, 3);
    let dl = rb.device_library.as_ref().unwrap();
    assert_eq!(dl.tracks.len(), 1);
    assert_eq!(dl.tracks[0].db_duration_secs, Some(20));

    let pdb = crate::library::pdb::parse_pdb(&fs::read(&report.pdb_path).unwrap()).unwrap();
    let analyze_path = &pdb.tracks[0].analyze_path;
    assert!(
        analyze_path.starts_with("/PIONEER/USBANLZ/P") && analyze_path.ends_with("/ANLZ0000.DAT"),
        "{analyze_path}"
    );
    assert_eq!(pdb.tracks[0].tempo, (bpm * 100.0).round() as u32);
    let dir = root.join(analyze_path.trim_start_matches('/')).parent().unwrap().to_path_buf();
    let dat = AnlzFile::read(&dir.join("ANLZ0000.DAT")).unwrap();
    assert_eq!(DatAnalysis::parse(&dat).unwrap().path, "/Contents/00000001/clicks.wav");
    let ext = AnlzFile::read(&dir.join("ANLZ0000.EXT")).unwrap();
    assert_eq!(ExtAnalysis::parse(&ext).unwrap().path, "/Contents/00000001/clicks.wav");
    let two_ex = AnlzFile::read(&dir.join("ANLZ0000.2EX")).unwrap();
    assert_eq!(ThreeBandAnalysis::parse(&two_ex).unwrap().path, "/Contents/00000001/clicks.wav");
}

#[test]
fn targets_with_an_export_or_unreadable_tracks_are_refused_and_leave_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(&src, &[("one.wav", "One", "A")], 1);

    for occupant in ["Contents", "PIONEER", ".boothready-export-stale"] {
        let occupied = tmp.path().join(format!("occupied-{occupant}"));
        fs::create_dir_all(occupied.join(occupant)).unwrap();
        let request = ExportRequest {
            root: occupied,
            tracks: tracks.clone(),
            playlists: vec![],
            analysis: Analysis::Skip,
            analysis_cache: None,
        };
        assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Target(_))), "{occupant}");
    }

    let root = tmp.path().join("stick");
    let request = ExportRequest {
        root: root.clone(),
        tracks: vec![tracks[0].clone(), src.join("missing.wav")],
        playlists: vec![],
        analysis: Analysis::Skip,
        analysis_cache: None,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Io { .. })));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "staging must be removed and nothing installed");

    let unsupported = src.join("odd.ogg");
    fs::write(&unsupported, b"OggS").unwrap();
    let request = ExportRequest {
        root: root.clone(),
        tracks: vec![unsupported],
        playlists: vec![],
        analysis: Analysis::Skip,
        analysis_cache: None,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Track(_))));

    let request = ExportRequest {
        root: root.clone(),
        tracks: vec![tracks[0].clone(), tracks[0].clone()],
        playlists: vec![],
        analysis: Analysis::Skip,
        analysis_cache: None,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Request(_))));
    let request = ExportRequest {
        root,
        tracks: tracks.clone(),
        playlists: vec![PlaylistSpec { name: "Bad".into(), tracks: vec![1] }],
        analysis: Analysis::Skip,
        analysis_cache: None,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Request(_))));
}

#[test]
fn file_names_are_made_safe_for_player_filesystems() {
    assert_eq!(safe_filename("a:b/c?d.mp3", 1), "a_b_c_d.mp3");
    assert_eq!(safe_filename("trailing dots... ", 1), "trailing dots");
    assert_eq!(safe_filename("\u{1}\u{2}", 7), "__");
    assert_eq!(safe_filename("   ", 7), "track-00000007");
    assert_eq!(safe_filename(&"x".repeat(300), 1).chars().count(), 180);
}

#[test]
fn civil_dates_match_the_calendar() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(10_956), "1999-12-31");
    assert_eq!(civil_date(11_016), "2000-02-29");
    assert_eq!(civil_date(19_782), "2024-02-29");
    assert_eq!(civil_date(19_783), "2024-03-01");
    assert_eq!(civil_date(20_744), "2026-10-18");
}

#[test]
fn a_cached_analysis_is_reused_and_repointed() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(&src, &[("clicks.wav", "Clicks", "Metronome")], 20);
    let cache = tmp.path().join("cache");
    let request = ExportRequest {
        root: tmp.path().join("first"),
        tracks: tracks.clone(),
        playlists: vec![PlaylistSpec { name: "Set".into(), tracks: vec![0] }],
        analysis: Analysis::Normal,
        analysis_cache: Some(cache.clone()),
    };
    let mut analysed = 0;
    let first = export_device_library(&request, |p| analysed = analysed.max(p.analysed)).unwrap();
    assert_eq!(analysed, 1);
    let store = super::analysis_cache::AnalysisCache::new(&cache);
    assert!(store.has(&tracks[0], djbooth_analyzer::RhythmMode::Normal));
    assert!(!store.has(&tracks[0], djbooth_analyzer::RhythmMode::Dynamic));

    // The second export reads the cache and still points the files at
    // their own media path, which differs because the file is track 2.
    let again = ExportRequest {
        root: tmp.path().join("second"),
        tracks: vec![source_tracks(&src, &[("other.wav", "Other", "Metronome")], 2)[0].clone(), tracks[0].clone()],
        playlists: vec![PlaylistSpec { name: "Set".into(), tracks: vec![1, 0] }],
        ..request.clone()
    };
    let mut analysed = 0;
    let second = export_device_library(&again, |p| analysed = analysed.max(p.analysed)).unwrap();
    assert_eq!(analysed, 1, "only the new track is analysed");
    assert_eq!(second.tracks[1].tempo_bpm, first.tracks[0].tempo_bpm);
    assert_eq!(second.tracks[1].key, first.tracks[0].key);
    let path = |root: &Path, t: &ExportedTrack| {
        let dat = fs::read(root.join("PIONEER/USBANLZ").join(usb_analysis_dir(t)).join("ANLZ0000.DAT")).unwrap();
        DatAnalysis::parse(&AnlzFile::parse(&dat).unwrap()).unwrap().path
    };
    assert_eq!(path(&first.root, &first.tracks[0]), "/Contents/00000001/clicks.wav");
    assert_eq!(path(&second.root, &second.tracks[1]), "/Contents/00000002/clicks.wav");

    // Stopping leaves the target untouched.
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let stopped =
        export_device_library_cancellable(&ExportRequest { root: tmp.path().join("third"), ..again }, &cancel, |_| {});
    assert!(matches!(stopped, Err(ExportError::Cancelled)));
    assert!(!tmp.path().join("third/Contents").exists());
    assert!(fs::read_dir(tmp.path().join("third")).unwrap().next().is_none());
}

/// The USBANLZ folder of a track, from its analysis path.
fn usb_analysis_dir(t: &ExportedTrack) -> PathBuf {
    let path = crate::library::pdb_writer::usb_analysis_path(&t.usb_path).unwrap();
    let dir = path.trim_start_matches("/PIONEER/USBANLZ/").rsplit_once('/').map(|(d, _)| d).unwrap().to_string();
    PathBuf::from(dir)
}
