use super::pdb::{parse_pdb, TABLE_ARTISTS, TABLE_PLAYLIST_ENTRIES, TABLE_PLAYLIST_TREE, TABLE_TRACKS};
use super::pdb_fixture::*;
use super::*;
use crate::audio::fixtures as audio;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Real rekordbox exports shipped inside the rekordcrate crate source.
fn rekordcrate_data() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".cargo")))?;
    for reg in fs::read_dir(home.join("registry/src")).ok()?.flatten() {
        let p = reg.path().join("rekordcrate-0.3.0/data");
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

#[test]
fn real_export_row_counts_match_rekordcrate_expectations() {
    let Some(data) = rekordcrate_data() else {
        eprintln!("skipping: rekordcrate test data not found");
        return;
    };
    let bytes = fs::read(data.join("pdb/num_rows/export.pdb")).unwrap();
    let pdb = parse_pdb(&bytes).unwrap();
    let count = |kind| pdb.row_counts.iter().find(|(k, _)| *k == kind).map(|(_, n)| *n).unwrap();
    // Expected values come from rekordcrate's own test suite, except the
    // playlist entries: rekordcrate 0.3.0 expects 6637 because it takes the
    // row count from one byte, while the 13-bit count in the page header
    // gives 7440, the same value the 11-bit present-row counts sum to.
    assert_eq!(count(TABLE_TRACKS), 3886);
    assert_eq!(count(TABLE_ARTISTS), 2216);
    assert_eq!(count(TABLE_PLAYLIST_TREE), 104);
    assert_eq!(count(TABLE_PLAYLIST_ENTRIES), 7440);
    assert_eq!(pdb.tracks.len(), 3886);
    assert_eq!(pdb.playlists.len(), 104);
    let entries: usize = pdb.playlists.iter().map(|p| p.track_ids.len()).sum();
    assert_eq!(entries, 7440);
    assert!(pdb.tracks.iter().all(|t| t.file_path.starts_with('/')), "paths should be volume-absolute");
    assert!(pdb.tracks.iter().filter(|t| t.artist.is_some()).count() > 3000);
    assert!(pdb.tracks.iter().all(|t| t.sample_rate == 0 || (8000..=192_000).contains(&t.sample_rate)));
}

#[test]
fn real_demo_export_scans_and_reports_missing_audio() {
    let Some(data) = rekordcrate_data() else {
        return;
    };
    let src = data.join("complete_export/demo_tracks");
    let vol = tempfile::tempdir().unwrap();
    copy_tree(&src, vol.path());
    let report = scan_libraries(vol.path());
    assert!(report.has(LibraryFormat::RekordboxDeviceLibrary));
    assert!(!report.has(LibraryFormat::RekordboxOneLibrary));
    let rb = report.rekordbox.as_ref().unwrap();
    let dl = rb.device_library.as_ref().unwrap();
    assert!(dl.parse_error.is_none());
    assert!(dl.has_ext);
    assert!(!dl.tracks.is_empty());
    assert!(rb.analysis_files >= 3);
    assert!(rb.has_settings);
    // rekordcrate ships the database but not the audio, so every track is missing.
    assert_eq!(dl.missing.len(), dl.tracks.len());
    assert!(report.problems().iter().any(|p| p.contains("missing")));

    let empty = scan_libraries(&data.join("complete_export/empty"));
    let dl = empty.rekordbox.unwrap().device_library.unwrap();
    assert!(dl.parse_error.is_none());
    assert!(dl.tracks.is_empty());
}

fn copy_tree(src: &Path, dst: &Path) {
    for e in walkdir::WalkDir::new(src).into_iter().flatten() {
        let rel = e.path().strip_prefix(src).unwrap();
        let to = dst.join(rel);
        if e.file_type().is_dir() {
            fs::create_dir_all(&to).unwrap();
        } else {
            fs::copy(e.path(), &to).unwrap();
        }
    }
}

fn track(id: u32, path: &str, size: u32, rate: u32, depth: u16) -> FixtureTrack {
    FixtureTrack {
        id,
        title: format!("Track {id}"),
        artist_id: 1 + id % 2,
        file_path: path.to_string(),
        sample_rate: rate,
        sample_depth: depth,
        bitrate: 1411,
        file_size: size,
        duration_secs: 300,
        tempo: 12800,
    }
}

/// A volume with a synthetic rekordbox export and real audio files.
pub(crate) fn build_rekordbox_volume(root: &Path) -> Vec<FixtureTrack> {
    let files: Vec<(&str, Vec<u8>, u32, u16)> = vec![
        ("Contents/Artist A/Warmup.mp3", audio::mp3_cbr(50), 44_100, 16),
        ("Contents/Artist A/Peak.wav", audio::wav(44_100, 16, 2, 4410, false), 44_100, 16),
        ("Contents/Artist B/Closing.flac", audio::flac(96_000, 24, 2, 96_000), 96_000, 24),
        ("Contents/Artist B/Hi-Res.aiff", audio::aiff(96_000, 24, 2, 960, false), 96_000, 24),
        ("Contents/Ünïcødé/Tëst.m4a", audio::m4a("alac", 44_100, 3), 44_100, 16),
    ];
    let mut tracks = Vec::new();
    for (i, (path, bytes, rate, depth)) in files.iter().enumerate() {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, bytes).unwrap();
        tracks.push(track(i as u32 + 1, &format!("/{path}"), bytes.len() as u32, *rate, *depth));
    }
    let playlists = vec![
        FixturePlaylist { id: 1, parent_id: 0, name: "Gigs".into(), is_folder: true, sort_order: 0, track_ids: vec![] },
        FixturePlaylist {
            id: 2,
            parent_id: 1,
            name: "Berghain".into(),
            is_folder: false,
            sort_order: 0,
            track_ids: vec![3, 1, 2],
        },
        FixturePlaylist {
            id: 3,
            parent_id: 0,
            name: "Warm-up ☀".into(),
            is_folder: false,
            sort_order: 1,
            track_ids: vec![1, 5],
        },
    ];
    let pdb = build_pdb(&tracks, &[(1, "Artist A".into()), (2, "Artist B".into())], &playlists);
    fs::create_dir_all(root.join("PIONEER/rekordbox")).unwrap();
    fs::write(root.join(DEVICE_LIBRARY_FILE), pdb).unwrap();
    tracks
}

#[test]
fn fixture_roundtrip_and_rekordcrate_agrees() {
    let tracks: Vec<FixtureTrack> = (1..=400)
        .map(|i| track(i, &format!("/Contents/Long Folder Name {i}/Some Track Title {i}.mp3"), i * 1000, 44_100, 16))
        .collect();
    let playlists = vec![FixturePlaylist {
        id: 9,
        parent_id: 0,
        name: "Everything".into(),
        is_folder: false,
        sort_order: 0,
        track_ids: (1..=400).rev().collect(),
    }];
    let bytes = build_pdb(&tracks, &[(1, "A".into()), (2, "Ä".into())], &playlists);
    let pdb = parse_pdb(&bytes).unwrap();
    assert!(pdb.warnings.is_empty(), "{:?}", pdb.warnings);
    assert_eq!(pdb.tracks.len(), 400);
    assert_eq!(pdb.tracks[41].file_path, "/Contents/Long Folder Name 42/Some Track Title 42.mp3");
    assert_eq!(pdb.tracks[1].artist.as_deref(), Some("A"));
    assert_eq!(pdb.tracks[0].artist.as_deref(), Some("Ä"));
    assert_eq!(pdb.playlists[0].track_ids, (1..=400).rev().collect::<Vec<_>>());

    // The same bytes through rekordcrate.
    use binrw::BinRead;
    use rekordcrate::pdb::{Header, PageType};
    let mut cur = std::io::Cursor::new(&bytes[..]);
    let header = Header::read(&mut cur).unwrap();
    let rows = |pt: PageType| -> usize {
        let t = header.tables.iter().find(|t| t.page_type == pt).unwrap();
        let mut cur = std::io::Cursor::new(&bytes[..]);
        header
            .read_pages(&mut cur, binrw::Endian::NATIVE, (&t.first_page, &t.last_page))
            .unwrap()
            .into_iter()
            .flat_map(|p| p.row_groups.into_iter())
            .map(|g| g.present_rows().count())
            .sum()
    };
    assert_eq!(rows(PageType::Tracks), 400);
    assert_eq!(rows(PageType::PlaylistEntries), 400);
    assert_eq!(rows(PageType::PlaylistTree), 1);
    assert_eq!(rows(PageType::Artists), 2);
}

/// Real rekordbox exports hold playlist entry pages with 284 rows. The page
/// header packs a 13-bit row-offset count into bytes 0x18..0x1B, so byte
/// 0x18 holds 28 and bit 0 of byte 0x19 carries the ninth bit, while the u16
/// at 0x22 is the index of the row the last transaction touched.
#[test]
fn playlist_entry_page_with_284_rows_is_read_in_full() {
    let tracks: Vec<FixtureTrack> =
        (1..=284).map(|i| track(i, &format!("/Contents/{i}.mp3"), 1000, 44_100, 16)).collect();
    let playlists = vec![FixturePlaylist {
        id: 3,
        parent_id: 0,
        name: "Big".into(),
        is_folder: false,
        sort_order: 0,
        track_ids: (1..=284).collect(),
    }];
    let mut bytes = build_pdb(&tracks, &[(1, "A".into())], &playlists);

    // The fixture writes the row count at 0x22 so that rekordcrate can read
    // its pages too. Give the entry page the transaction fields a real export
    // carries instead, so only the packed count at 0x18 says how many rows
    // there are: page 145 of rekordcrate's `num_rows` export has 284 rows
    // and a last transaction that touched one row at index 60.
    const PAGE: usize = 4096;
    let entry_pages: Vec<usize> = (1..bytes.len() / PAGE)
        .filter(|&i| {
            let p = &bytes[i * PAGE..(i + 1) * PAGE];
            u32::from_le_bytes(p[8..12].try_into().unwrap()) == TABLE_PLAYLIST_ENTRIES && p[0x1B] & 0x40 == 0
        })
        .collect();
    assert_eq!(entry_pages.len(), 1, "284 twelve-byte rows fill exactly one 4 KiB page");
    let o = entry_pages[0] * PAGE;
    assert_eq!(bytes[o + 0x18], 28);
    assert_eq!(bytes[o + 0x19] & 1, 1);
    bytes[o + 0x20..o + 0x22].copy_from_slice(&1u16.to_le_bytes());
    bytes[o + 0x22..o + 0x24].copy_from_slice(&60u16.to_le_bytes());

    let check = |bytes: &[u8]| {
        let pdb = parse_pdb(bytes).unwrap();
        assert!(pdb.warnings.is_empty(), "{:?}", pdb.warnings);
        let count = pdb.row_counts.iter().find(|(k, _)| *k == TABLE_PLAYLIST_ENTRIES).map(|(_, n)| *n).unwrap();
        assert_eq!(count, 284);
        assert_eq!(pdb.playlists[0].track_ids, (1..=284).collect::<Vec<_>>());
    };
    check(&bytes);

    // A failed last transaction leaves 0x1FFF in both transaction fields.
    bytes[o + 0x20..o + 0x22].copy_from_slice(&0x1FFFu16.to_le_bytes());
    bytes[o + 0x22..o + 0x24].copy_from_slice(&0x1FFFu16.to_le_bytes());
    check(&bytes);
}

#[test]
fn scan_valid_fixture_volume() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    let report = scan_libraries(vol.path());
    assert_eq!(report.formats, vec![LibraryFormat::RekordboxDeviceLibrary]);
    let dl = report.rekordbox.as_ref().unwrap().device_library.as_ref().unwrap();
    assert_eq!(dl.tracks.len(), 5);
    assert!(dl.missing.is_empty(), "{:?}", dl.missing);
    assert!(dl.size_mismatch.is_empty());
    let names: Vec<&str> = dl.playlists.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"Berghain") && names.contains(&"Warm-up ☀"));
    assert!(report.problems().is_empty(), "{:?}", report.problems());
    assert_eq!(report.referenced_paths().len(), 5);
}

#[test]
fn missing_changed_and_case_mismatched_files() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    fs::remove_file(vol.path().join("Contents/Artist A/Peak.wav")).unwrap();
    let mut edited = audio::flac(44_100, 16, 2, 10);
    edited.extend_from_slice(&[0u8; 333]);
    fs::write(vol.path().join("Contents/Artist B/Closing.flac"), edited).unwrap();
    // Players on FAT/exFAT resolve paths case-insensitively; so do we.
    fs::rename(vol.path().join("Contents/Artist B/Hi-Res.aiff"), vol.path().join("Contents/Artist B/HI-RES.AIFF"))
        .unwrap();
    let report = scan_libraries(vol.path());
    let dl = report.rekordbox.as_ref().unwrap().device_library.as_ref().unwrap();
    assert_eq!(dl.missing, vec!["Contents/Artist A/Peak.wav".to_string()]);
    assert_eq!(dl.size_mismatch, vec!["Contents/Artist B/Closing.flac".to_string()]);
    // The drive summary words size differences once it has the scanned audio.
    assert_eq!(report.problems().len(), 1);
}

fn exported(duration: u16, bitrate: u32, rate: u32) -> LibraryTrack {
    LibraryTrack {
        id: 1,
        title: "t".into(),
        artist: None,
        path: "Contents/t.mp3".into(),
        db_sample_rate: Some(rate),
        db_bit_depth: None,
        db_file_size: Some(1),
        db_duration_secs: Some(duration),
        db_bitrate_kbps: Some(bitrate),
    }
}

#[test]
fn a_size_change_with_the_same_audio_is_a_tag_change() {
    let mut info = crate::audio::probe_reader(std::io::Cursor::new(audio::mp3_cbr(400)), 400 * 1044).unwrap();
    let secs = info.duration_secs.unwrap().round() as u16;
    let rate = info.sample_rate.unwrap();
    let kbps = info.bitrate_kbps.unwrap();
    assert_eq!(classify_size_change(&exported(secs, kbps, rate), Some(&info)), SizeChange::TagsOnly);
    // A different length, sample rate or constant bitrate means a different file.
    assert_eq!(classify_size_change(&exported(secs + 20, kbps, rate), Some(&info)), SizeChange::Replaced);
    assert_eq!(classify_size_change(&exported(secs, kbps, 48_000), Some(&info)), SizeChange::Replaced);
    assert_eq!(classify_size_change(&exported(secs, kbps + 64, rate), Some(&info)), SizeChange::Replaced);
    // A variable bitrate is not compared, since rekordbox records an average.
    info.vbr = true;
    assert_eq!(classify_size_change(&exported(secs, kbps + 64, rate), Some(&info)), SizeChange::TagsOnly);
    // Nothing to compare against is treated as a replaced file.
    assert_eq!(classify_size_change(&exported(secs, kbps, rate), None), SizeChange::Replaced);
    let mut unknown = exported(secs, kbps, rate);
    unknown.db_duration_secs = None;
    assert_eq!(classify_size_change(&unknown, Some(&info)), SizeChange::Replaced);
}

#[test]
fn serato_data_in_a_mismatched_file_is_noticed() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    let path = vol.path().join("Contents/Artist B/Closing.flac");
    let mut tagged = b"ID3\x04\x00\x00\x00\x00\x00\x20".to_vec();
    tagged.extend_from_slice(b"GEOB....Serato Markers2.........");
    tagged.extend_from_slice(&fs::read(&path).unwrap());
    fs::write(&path, tagged).unwrap();
    let report = scan_libraries(vol.path());
    let dl = report.rekordbox.as_ref().unwrap().device_library.as_ref().unwrap();
    assert_eq!(dl.size_mismatch, vec!["Contents/Artist B/Closing.flac".to_string()]);
    assert_eq!(dl.serato_tagged, dl.size_mismatch);
}

fn set_modified(path: &Path, when: SystemTime) {
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

#[test]
fn one_library_detection_and_staleness() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    let ol = vol.path().join(ONE_LIBRARY_FILE);
    fs::write(&ol, (0..4096u32).map(|i| (i * 7 % 251) as u8).collect::<Vec<_>>()).unwrap();
    let report = scan_libraries(vol.path());
    assert!(report.has(LibraryFormat::RekordboxOneLibrary));
    let rb = report.rekordbox.as_ref().unwrap();
    assert!(rb.one_library.as_ref().unwrap().encrypted);
    assert!(rb.warnings.is_empty());
    assert_eq!(rb.lag, None);

    let days = |n: u64| SystemTime::now() - Duration::from_secs(n * 24 * 3600);
    set_modified(&ol, days(3));
    let report = scan_libraries(vol.path());
    let lag = report.rekordbox.as_ref().unwrap().lag.unwrap();
    assert_eq!((lag.older, lag.newer), (LibraryFormat::RekordboxOneLibrary, LibraryFormat::RekordboxDeviceLibrary));
    assert!(report.problems().iter().any(|p| p.contains("about 3 days before")), "{:?}", report.problems());

    // Changes still sitting in the SQLite WAL count as written.
    fs::write(vol.path().join(ONE_LIBRARY_WAL_FILE), b"wal").unwrap();
    assert_eq!(scan_libraries(vol.path()).rekordbox.unwrap().lag, None);

    set_modified(&vol.path().join(DEVICE_LIBRARY_FILE), days(40));
    let lag = scan_libraries(vol.path()).rekordbox.unwrap().lag.unwrap();
    assert_eq!(lag.older, LibraryFormat::RekordboxDeviceLibrary);
}

/// A real rekordbox export carries a fixed date on export.pdb while
/// exportExt.pdb, written by the same export, has the true one.
#[test]
fn placeholder_date_on_export_pdb_is_not_a_stale_library() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    fs::write(vol.path().join(ONE_LIBRARY_FILE), [7u8; 4096]).unwrap();
    fs::write(vol.path().join(DEVICE_LIBRARY_EXT_FILE), b"ext").unwrap();
    let jan_2024 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_060_000);
    set_modified(&vol.path().join(DEVICE_LIBRARY_FILE), jan_2024);

    let report = scan_libraries(vol.path());
    let rb = report.rekordbox.as_ref().unwrap();
    assert_eq!(rb.lag, None, "{:?}", report.problems());
    assert!(rb.device_library.as_ref().unwrap().modified_unix.unwrap() > 1_704_060_000 + 3600);
}

#[test]
fn damaged_pdb_is_reported_not_panicked() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    fs::write(vol.path().join(DEVICE_LIBRARY_FILE), b"not a database at all, just text").unwrap();
    let report = scan_libraries(vol.path());
    let dl = report.rekordbox.unwrap().device_library.unwrap();
    assert!(dl.parse_error.is_some());

    // Random mutations of a valid database must never panic.
    let tracks: Vec<FixtureTrack> = (1..=40).map(|i| track(i, &format!("/C/{i}.mp3"), 1, 44_100, 16)).collect();
    let good = build_pdb(&tracks, &[(1, "A".into())], &[]);
    let mut x = 0xDEAD_BEEFu32;
    for _ in 0..4000 {
        let mut m = good.clone();
        for _ in 0..4 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let i = x as usize % m.len();
            m[i] = (x >> 11) as u8;
        }
        let _ = parse_pdb(&m);
    }
}

#[cfg(feature = "sqlite")]
#[test]
fn engine_database_validation() {
    let vol = tempfile::tempdir().unwrap();
    let root = vol.path();
    fs::create_dir_all(root.join("Engine Library/Database2")).unwrap();
    fs::create_dir_all(root.join("Engine Library/Music/X")).unwrap();
    fs::write(root.join("Engine Library/Music/X/one.mp3"), audio::mp3_cbr(10)).unwrap();
    let conn = rusqlite::Connection::open(root.join(ENGINE_DB_FILE)).unwrap();
    conn.execute_batch(
        "CREATE TABLE Information (id INTEGER PRIMARY KEY, uuid TEXT, schemaVersionMajor INTEGER, schemaVersionMinor INTEGER, schemaVersionPatch INTEGER);
         INSERT INTO Information VALUES (1, 'u', 2, 20, 3);
         CREATE TABLE Track (id INTEGER PRIMARY KEY, path TEXT, filename TEXT, title TEXT);
         INSERT INTO Track VALUES (1, '../Music/X/one.mp3', 'one.mp3', 'One');
         INSERT INTO Track VALUES (2, '../Music/X/two.mp3', 'two.mp3', 'Two');
         INSERT INTO Track VALUES (3, '/Users/dj/Music/three.mp3', 'three.mp3', 'Three');
         CREATE TABLE Playlist (id INTEGER PRIMARY KEY, title TEXT, parentListId INTEGER);
         INSERT INTO Playlist VALUES (1, 'Techno', 0);
         CREATE TABLE PlaylistEntity (id INTEGER PRIMARY KEY, listId INTEGER, trackId INTEGER);
         INSERT INTO PlaylistEntity VALUES (1, 1, 2);
         INSERT INTO PlaylistEntity VALUES (2, 1, 1);",
    )
    .unwrap();
    drop(conn);
    let before: Vec<_> =
        fs::read_dir(root.join("Engine Library/Database2")).unwrap().flatten().map(|e| e.file_name()).collect();
    let report = scan_libraries(root);
    assert!(report.has(LibraryFormat::EngineDatabase));
    let e = report.engine.as_ref().unwrap();
    assert_eq!(e.error, None);
    assert_eq!(e.schema_version.as_deref(), Some("2.20.3"));
    assert_eq!(e.track_count, 3);
    assert_eq!(e.track_paths, vec!["Engine Library/Music/X/one.mp3".to_string()]);
    assert_eq!(e.missing, vec!["Engine Library/Music/X/two.mp3".to_string()]);
    assert_eq!(e.outside_drive, 1);
    assert_eq!(e.playlists[0].track_ids, vec![2, 1]);
    // Read-only inspection must not leave journal files on the drive.
    let after: Vec<_> =
        fs::read_dir(root.join("Engine Library/Database2")).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(before, after);
}

#[test]
fn walker_skips_litter_and_counts_apple_double() {
    let vol = tempfile::tempdir().unwrap();
    build_rekordbox_volume(vol.path());
    fs::write(vol.path().join("Contents/Artist A/._Peak.wav"), [0u8; 4096]).unwrap();
    fs::create_dir_all(vol.path().join(".Trashes/501")).unwrap();
    fs::write(vol.path().join(".Trashes/501/old.mp3"), audio::mp3_cbr(2)).unwrap();
    let files: Vec<_> = walk_audio_files(vol.path()).map(|(rel, _, _)| rel).collect();
    assert_eq!(files.len(), 5, "{files:?}");
    assert_eq!(count_apple_double(vol.path()), 1);
}
