//! Write a rekordbox OneLibrary database (`PIONEER/rekordbox/exportLibrary.db`).
//!
//! Ported from djbooth's `writeOneLibraryPlaylist`
//! (`packages/rekordbox-ts/src/devicelib-plus/export.ts`, MIT, Dylan Jones
//! and Georgios Konstantopoulos) at commit 452c46f. The schema and the fixed
//! `menuItem`, `category`, `sort` and `color` rows are djbooth's, which match
//! the databases rekordbox wrote to two real sticks statement for statement.
//! Local changes, also recorded in `crates/boothready-core/UPSTREAM.md`:
//!
//! - One call builds the whole database from the same [`TrackInput`] rows
//!   the Device Library writer receives, so both databases describe the same
//!   files with the same IDs. djbooth appends one folder and one playlist per
//!   call to an existing file. Playlists sit at the root, as in `export.pdb`.
//! - Album, genre and label rows are written from the tags as well as artist
//!   and key, so OneLibrary carries the same browse data as `export.pdb`.
//! - The per-track bookkeeping values, the four My Tag parent rows and
//!   `property.dbVersion` follow what rekordbox itself wrote in the 2025
//!   database on Jommi's stick (`analysedBits` 41, `contentLink` 788224, null
//!   update counters, `dbVersion` "1000"), where djbooth's differ.
//! - The journal mode is `DELETE`, so the tree holds one file with no `-wal`
//!   or `-shm` beside it.

use super::onelibrary::{unlock, OneLibraryError};
use super::pdb_writer::{file_type_for_path, TrackInput};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::HashMap;
use std::path::Path;

const SCHEMA: &str = r"
CREATE TABLE album(album_id integer primary key, name varchar, artist_id integer, image_id integer, isComplation integer, nameForSearch varchar);
CREATE TABLE artist(artist_id integer primary key, name varchar, nameForSearch varchar);
CREATE TABLE category(category_id integer primary key, menuItem_id integer, sequenceNo integer, isVisible integer);
CREATE TABLE color(color_id integer primary key, name varchar);
CREATE TABLE content(content_id integer primary key, title varchar, titleForSearch varchar, subtitle varchar, bpmx100 integer, length integer, trackNo integer, discNo integer, artist_id_artist integer, artist_id_remixer integer, artist_id_originalArtist integer, artist_id_composer integer, artist_id_lyricist integer, album_id integer, genre_id integer, label_id integer, key_id integer, color_id integer, image_id integer, djComment varchar, rating integer, releaseYear integer, releaseDate varchar, dateCreated varchar, dateAdded varchar, path varchar, fileName varchar, fileSize integer, fileType integer, bitrate integer, bitDepth integer, samplingRate integer, isrc varchar, djPlayCount integer, isHotCueAutoLoadOn integer, isKuvoDeliverStatusOn integer, kuvoDeliveryComment varchar, masterDbId integer, masterContentId integer, analysisDataFilePath varchar, analysedBits integer, contentLink integer, hasModified integer, cueUpdateCount integer, analysisDataUpdateCount integer, informationUpdateCount integer);
CREATE TABLE cue(cue_id integer primary key, content_id integer, kind integer, colorTableIndex integer, cueComment varchar, isActiveLoop integer, beatLoopNumerator integer, beatLoopDenominator integer, inUsec integer, outUsec integer, in150FramePerSec integer, out150FramePerSec integer, inMpegFrameNumber integer, outMpegFrameNumber integer, inMpegAbs integer, outMpegAbs integer, inDecodingStartFramePosition integer, outDecodingStartFramePosition integer, inFileOffsetInBlock integer, OutFileOffsetInBlock integer, inNumberOfSampleInBlock integer, outNumberOfSampleInBlock integer);
CREATE TABLE genre(genre_id integer primary key, name varchar);
CREATE TABLE history(history_id integer primary key, sequenceNo integer, name varchar, attribute integer, history_id_parent integer);
CREATE TABLE history_content(history_id integer, content_id integer, sequenceNo integer);
CREATE TABLE hotCueBankList(hotCueBankList_id integer primary key, sequenceNo integer, name varchar, image_id integer, attribute integer, hotCueBankList_id_parent integer);
CREATE TABLE hotCueBankList_cue(hotCueBankList_id integer, cue_id integer, sequenceNo integer);
CREATE TABLE image(image_id integer primary key, path varchar);
CREATE TABLE key(key_id integer primary key, name varchar);
CREATE TABLE label(label_id integer primary key, name varchar);
CREATE TABLE menuItem(menuItem_id integer primary key, kind integer, name varchar);
CREATE TABLE myTag(myTag_id integer primary key, sequenceNo integer, name varchar, attribute integer, myTag_id_parent integer);
CREATE TABLE myTag_content(myTag_id integer, content_id integer);
CREATE TABLE playlist(playlist_id integer primary key, sequenceNo integer, name varchar, image_id integer, attribute integer, playlist_id_parent integer);
CREATE TABLE playlist_content(playlist_id integer, content_id integer, sequenceNo integer);
CREATE TABLE property(deviceName varchar, dbVersion varchar, numberOfContents integer, createdDate varchar, backGroundColorType integer, myTagMasterDBID integer);
CREATE TABLE recommendedLike(content_id_1 integer, content_id_2 integer, rating integer, createdDate integer);
CREATE TABLE sort(sort_id integer primary key, menuItem_id integer, sequenceNo integer, isVisible integer, isSelectedAsSubColumn integer);
CREATE INDEX index_hotCueBankList_cue_hotCueBankList_id on hotCueBankList_cue(hotCueBankList_id);
CREATE INDEX index_myTag_content_content_id on myTag_content(content_id);
CREATE INDEX index_myTag_content_myTag_id on myTag_content(myTag_id);
CREATE INDEX index_playlist_content_playlist_id on playlist_content(playlist_id);
";

const MENU_ITEMS: [(u32, u32, &str); 27] = [
    (1, 128, "\u{FFFA}GENRE\u{FFFB}"),
    (2, 129, "\u{FFFA}ARTIST\u{FFFB}"),
    (3, 130, "\u{FFFA}ALBUM\u{FFFB}"),
    (4, 131, "\u{FFFA}TRACK\u{FFFB}"),
    (5, 133, "\u{FFFA}BPM\u{FFFB}"),
    (6, 134, "\u{FFFA}RATING\u{FFFB}"),
    (7, 135, "\u{FFFA}YEAR\u{FFFB}"),
    (8, 136, "\u{FFFA}REMIXER\u{FFFB}"),
    (9, 137, "\u{FFFA}LABEL\u{FFFB}"),
    (10, 138, "\u{FFFA}ORIGINAL ARTIST\u{FFFB}"),
    (11, 139, "\u{FFFA}KEY\u{FFFB}"),
    (12, 141, "\u{FFFA}CUE\u{FFFB}"),
    (13, 142, "\u{FFFA}COLOR\u{FFFB}"),
    (14, 146, "\u{FFFA}TIME\u{FFFB}"),
    (15, 147, "\u{FFFA}BITRATE\u{FFFB}"),
    (16, 148, "\u{FFFA}FILE NAME\u{FFFB}"),
    (17, 132, "\u{FFFA}PLAYLIST\u{FFFB}"),
    (18, 152, "\u{FFFA}HOT CUE BANK\u{FFFB}"),
    (19, 149, "\u{FFFA}HISTORY\u{FFFB}"),
    (20, 145, "\u{FFFA}SEARCH\u{FFFB}"),
    (21, 150, "\u{FFFA}COMMENTS\u{FFFB}"),
    (22, 140, "\u{FFFA}DATE ADDED\u{FFFB}"),
    (23, 151, "\u{FFFA}DJ PLAY COUNT\u{FFFB}"),
    (24, 144, "\u{FFFA}FOLDER\u{FFFB}"),
    (25, 161, "\u{FFFA}DEFAULT\u{FFFB}"),
    (26, 162, "\u{FFFA}ALPHABET\u{FFFB}"),
    (27, 170, "\u{FFFA}MATCHING\u{FFFB}"),
];

const CATEGORIES: [(u32, u32, u32, u32); 22] = [
    (1, 1, 0, 0),
    (2, 2, 1, 1),
    (3, 3, 2, 1),
    (4, 4, 3, 1),
    (5, 17, 5, 1),
    (6, 5, 0, 0),
    (7, 6, 0, 0),
    (8, 7, 0, 0),
    (9, 8, 0, 0),
    (10, 9, 0, 0),
    (11, 10, 0, 0),
    (12, 11, 4, 1),
    (15, 13, 0, 0),
    (17, 24, 9, 1),
    (18, 20, 7, 1),
    (19, 14, 0, 0),
    (20, 15, 0, 0),
    (21, 16, 0, 0),
    (22, 19, 6, 1),
    (23, 18, 0, 0),
    (26, 27, 8, 1),
    (27, 22, 10, 1),
];

const SORTS: [(u32, u32, u32, u32, u32); 17] = [
    (0, 25, 1, 1, 0),
    (1, 26, 2, 1, 0),
    (2, 2, 3, 1, 0),
    (3, 3, 4, 1, 0),
    (4, 5, 5, 1, 0),
    (5, 6, 6, 1, 0),
    (6, 1, 0, 0, 0),
    (7, 21, 0, 0, 0),
    (8, 14, 0, 0, 0),
    (9, 8, 0, 0, 0),
    (10, 9, 0, 0, 0),
    (11, 10, 0, 0, 0),
    (12, 11, 7, 1, 0),
    (13, 15, 0, 0, 0),
    (15, 13, 0, 0, 0),
    (16, 23, 0, 0, 0),
    (17, 22, 0, 0, 0),
];

const COLORS: [&str; 8] = ["Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple"];
/// The four My Tag columns rekordbox writes, without its 24 default tags.
const MY_TAG_PARENTS: [&str; 4] = ["Genre", "Components", "Situation", "Untitled Column"];
/// `property.dbVersion` as rekordbox wrote it in December 2025.
pub const DB_VERSION: &str = "1000";
/// `content.analysedBits` and `content.contentLink` as rekordbox wrote them
/// for every track of the inspected exports with ordinary analysis.
const ANALYSED_BITS: i64 = 41;
const CONTENT_LINK: i64 = 788_224;

/// One playlist: its name and the IDs of its tracks in playing order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistInput {
    pub name: String,
    pub track_ids: Vec<u32>,
}

/// Counts of what was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    pub tracks: usize,
    pub playlists: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum OneLibraryWriteError {
    #[error("{0} already exists; a OneLibrary database is always written whole")]
    Exists(String),
    #[error("{0}")]
    Input(String),
    #[error("{0}")]
    Library(#[from] OneLibraryError),
    #[error("{0}")]
    Sql(#[from] rusqlite::Error),
}

/// Write a new encrypted OneLibrary database at `path` holding `tracks` and
/// `playlists`. Every track needs an ID, and each playlist may only refer to
/// those IDs. `created` is the `YYYY-MM-DD` date stored in `property`.
pub fn write_one_library(
    path: &Path,
    tracks: &[TrackInput],
    playlists: &[PlaylistInput],
    created: &str,
) -> Result<Written, OneLibraryWriteError> {
    if path.exists() {
        return Err(OneLibraryWriteError::Exists(path.display().to_string()));
    }
    let mut ids = HashMap::new();
    for t in tracks {
        let id = t.id.ok_or_else(|| OneLibraryWriteError::Input(format!("track '{}' has no ID", t.title)))?;
        if ids.insert(id, ()).is_some() {
            return Err(OneLibraryWriteError::Input(format!("track ID {id} is used twice")));
        }
    }
    for p in playlists {
        if let Some(missing) = p.track_ids.iter().find(|id| !ids.contains_key(id)) {
            return Err(OneLibraryWriteError::Input(format!(
                "playlist '{}' refers to unknown track {missing}",
                p.name
            )));
        }
    }

    let mut conn = Connection::open(path)?;
    unlock(&conn)?;
    conn.execute_batch("PRAGMA journal_mode = DELETE;")?;
    let tx = conn.transaction()?;
    initialize(&tx, created, tracks.len())?;
    let mut names = Names::default();
    for t in tracks {
        insert_content(&tx, &mut names, t, created)?;
    }
    for (index, p) in playlists.iter().enumerate() {
        let playlist_id = index as i64 + 1;
        tx.execute("INSERT INTO playlist VALUES (?1, ?2, ?3, NULL, 0, 0)", params![playlist_id, index as i64, p.name])?;
        let mut member = tx.prepare_cached("INSERT INTO playlist_content VALUES (?1, ?2, ?3)")?;
        for (position, id) in p.track_ids.iter().enumerate() {
            member.execute(params![playlist_id, id, position as i64 + 1])?;
        }
    }
    tx.commit()?;
    conn.close().map_err(|(_, e)| e)?;
    Ok(Written { tracks: tracks.len(), playlists: playlists.len() })
}

fn initialize(tx: &Transaction, created: &str, contents: usize) -> rusqlite::Result<()> {
    tx.execute_batch(SCHEMA)?;
    for (id, kind, name) in MENU_ITEMS {
        tx.execute("INSERT INTO menuItem VALUES (?1, ?2, ?3)", params![id, kind, name])?;
    }
    for (id, menu, seq, visible) in CATEGORIES {
        tx.execute("INSERT INTO category VALUES (?1, ?2, ?3, ?4)", params![id, menu, seq, visible])?;
    }
    for (id, menu, seq, visible, sub) in SORTS {
        tx.execute("INSERT INTO sort VALUES (?1, ?2, ?3, ?4, ?5)", params![id, menu, seq, visible, sub])?;
    }
    for (index, name) in COLORS.iter().enumerate() {
        tx.execute("INSERT INTO color VALUES (?1, ?2)", params![index as i64 + 1, name])?;
    }
    for (index, name) in MY_TAG_PARENTS.iter().enumerate() {
        tx.execute("INSERT INTO myTag VALUES (?1, ?2, ?3, 1, 0)", params![index as i64 + 1, index as i64, name])?;
    }
    tx.execute("INSERT INTO property VALUES ('', ?1, ?2, ?3, 0, 1)", params![DB_VERSION, contents as i64, created])?;
    Ok(())
}

/// IDs of the lookup rows written so far, by table and name.
#[derive(Default)]
struct Names(HashMap<(&'static str, String), i64>);

impl Names {
    fn id(&mut self, tx: &Transaction, table: &'static str, name: Option<&str>) -> rusqlite::Result<Option<i64>> {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else { return Ok(None) };
        if let Some(&id) = self.0.get(&(table, name.to_string())) {
            return Ok(Some(id));
        }
        let id = self.0.keys().filter(|(t, _)| *t == table).count() as i64 + 1;
        let sql = match table {
            "artist" => "INSERT INTO artist VALUES (?1, ?2, NULL)",
            "album" => "INSERT INTO album VALUES (?1, ?2, NULL, NULL, 0, NULL)",
            "genre" => "INSERT INTO genre VALUES (?1, ?2)",
            "label" => "INSERT INTO label VALUES (?1, ?2)",
            _ => "INSERT INTO key VALUES (?1, ?2)",
        };
        tx.execute(sql, params![id, name])?;
        self.0.insert((table, name.to_string()), id);
        Ok(Some(id))
    }
}

fn insert_content(
    tx: &Transaction,
    names: &mut Names,
    t: &TrackInput,
    created: &str,
) -> Result<(), OneLibraryWriteError> {
    let id = t.id.ok_or_else(|| OneLibraryWriteError::Input(format!("track '{}' has no ID", t.title)))?;
    let artist = names.id(tx, "artist", t.artist.as_deref())?;
    let album = names.id(tx, "album", t.album.as_deref())?;
    let genre = names.id(tx, "genre", t.genre.as_deref())?;
    let label = names.id(tx, "label", t.label.as_deref())?;
    let key = names.id(tx, "key", t.key.as_deref())?;
    let file_name =
        t.filename.clone().unwrap_or_else(|| t.file_path.rsplit('/').next().unwrap_or(&t.file_path).to_string());
    let file_type = t
        .file_type
        .or_else(|| file_type_for_path(&t.file_path))
        .ok_or_else(|| OneLibraryWriteError::Input(format!("'{}' is not a format OneLibrary can list", t.file_path)))?;
    let added = if t.date_added.is_empty() { created } else { t.date_added.as_str() };
    tx.prepare_cached(
        "INSERT INTO content (
            content_id, title, titleForSearch, subtitle, bpmx100, length, trackNo, discNo,
            artist_id_artist, artist_id_remixer, artist_id_originalArtist, artist_id_composer,
            artist_id_lyricist, album_id, genre_id, label_id, key_id, color_id, image_id,
            djComment, rating, releaseYear, releaseDate, dateCreated, dateAdded, path, fileName,
            fileSize, fileType, bitrate, bitDepth, samplingRate, isrc, djPlayCount,
            isHotCueAutoLoadOn, isKuvoDeliverStatusOn, kuvoDeliveryComment, masterDbId,
            masterContentId, analysisDataFilePath, analysedBits, contentLink, hasModified,
            cueUpdateCount, analysisDataUpdateCount, informationUpdateCount
        ) VALUES (
            ?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, 0, ?9, ?10, ?11, ?12, 0, NULL,
            ?13, ?14, ?15, ?16, ?17, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, '', ?25,
            1, 1, '', NULL, NULL, ?26, ?27, ?28, 0, NULL, NULL, NULL
        )",
    )?
    .execute(params![
        id,
        t.title,
        t.mix_name,
        t.tempo,
        t.duration,
        t.track_number,
        t.disc_number,
        artist,
        album,
        genre,
        label,
        key,
        t.comment,
        t.rating,
        t.year,
        t.release_date,
        added,
        t.file_path,
        file_name,
        t.file_size,
        file_type,
        t.bitrate,
        t.sample_depth,
        t.sample_rate,
        t.play_count,
        t.analyze_path,
        ANALYSED_BITS,
        CONTENT_LINK,
    ])?;
    Ok(())
}

/// The `dbVersion` stored in an existing database, for tests and checks.
pub fn stored_db_version(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT dbVersion FROM property", [], |r| r.get(0)).optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::onelibrary::{open_one_library, read_one_library_file};

    fn track(id: u32, title: &str, artist: &str, key: &str) -> TrackInput {
        TrackInput {
            id: Some(id),
            title: title.into(),
            file_path: format!("/Contents/{id:08X}/{title}.mp3"),
            artist: Some(artist.into()),
            album: Some("Album".into()),
            genre: Some("Techno".into()),
            key: Some(key.into()),
            analyze_path: format!("/PIONEER/USBANLZ/P000/{id:08X}/ANLZ0000.DAT"),
            date_added: "2026-10-19".into(),
            tempo: 13_500,
            duration: 360,
            bitrate: 320,
            sample_rate: 44_100,
            sample_depth: 16,
            file_size: 14_000_000,
            ..TrackInput::default()
        }
    }

    #[test]
    fn a_written_database_reads_back_with_the_same_tracks_and_playlists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("exportLibrary.db");
        let tracks = vec![
            track(1, "First", "Artist A", "Am"),
            track(2, "Second", "Artist B", "Fm"),
            track(3, "Third", "Artist A", "Am"),
        ];
        let playlists = vec![
            PlaylistInput { name: "Warmup".into(), track_ids: vec![3, 1] },
            PlaylistInput { name: "Peak".into(), track_ids: vec![2, 3, 1] },
        ];
        let written = write_one_library(&path, &tracks, &playlists, "2026-10-19").expect("write");
        assert_eq!(written, Written { tracks: 3, playlists: 2 });

        let head = std::fs::read(&path).expect("read");
        assert_ne!(&head[..16], b"SQLite format 3\0", "the file must be encrypted");
        assert!(!dir.path().join("exportLibrary.db-wal").exists());

        let lib = read_one_library_file(&path).expect("read back");
        assert_eq!(lib.db_version.as_deref(), Some(DB_VERSION));
        assert_eq!(lib.tracks.len(), 3);
        let first = &lib.tracks[0];
        assert_eq!((first.id, first.title.as_str(), first.artist.as_deref()), (1, "First", Some("Artist A")));
        assert_eq!(
            (first.key.as_deref(), first.album.as_deref(), first.genre.as_deref()),
            (Some("Am"), Some("Album"), Some("Techno"))
        );
        assert_eq!(first.file_path, "/Contents/00000001/First.mp3");
        assert_eq!(first.file_name, "First.mp3");
        assert_eq!(first.analyze_path, "/PIONEER/USBANLZ/P000/00000001/ANLZ0000.DAT");
        assert_eq!((first.tempo, first.duration_secs, first.file_type), (13_500, 360, 1));
        let names: Vec<_> = lib.playlists.iter().map(|p| (p.name.as_str(), p.track_ids.clone(), p.is_folder)).collect();
        assert_eq!(names, vec![("Warmup", vec![3, 1], false), ("Peak", vec![2, 3, 1], false)]);

        // Shared lookup rows: two artists, two keys.
        let conn = open_one_library(&path).expect("open");
        let count =
            |t: &str| conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get::<_, i64>(0)).expect("count");
        assert_eq!(
            (count("artist"), count("key"), count("menuItem"), count("category"), count("sort")),
            (2, 2, 27, 22, 17)
        );
        let bits: (i64, i64) = conn
            .query_row("SELECT analysedBits, contentLink FROM content LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("bits");
        assert_eq!(bits, (ANALYSED_BITS, CONTENT_LINK));
    }

    #[test]
    fn an_existing_file_is_never_appended_to() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("exportLibrary.db");
        std::fs::write(&path, b"x").expect("write");
        assert!(matches!(write_one_library(&path, &[], &[], "2026-10-19"), Err(OneLibraryWriteError::Exists(_))));
    }

    #[test]
    fn playlists_may_only_refer_to_written_tracks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("exportLibrary.db");
        let err = write_one_library(
            &path,
            &[track(1, "One", "A", "Am")],
            &[PlaylistInput { name: "P".into(), track_ids: vec![2] }],
            "2026-10-19",
        );
        assert!(matches!(err, Err(OneLibraryWriteError::Input(_))));
        assert!(!path.exists());
    }

    /// Compares a generated database with one rekordbox wrote. Run with
    /// `ONELIBRARY_REFERENCE=<path to exportLibrary.db> cargo test -p boothready-core -- --ignored rekordbox_reference`.
    /// The reference holds personal track names, so it never enters the repository.
    #[test]
    #[ignore = "needs a rekordbox-written exportLibrary.db in ONELIBRARY_REFERENCE"]
    fn rekordbox_reference_has_the_same_schema_and_fixed_rows() {
        let Some(reference) = std::env::var_os("ONELIBRARY_REFERENCE") else { return };
        let dir = tempfile::tempdir().expect("tempdir");
        let ours = dir.path().join("exportLibrary.db");
        write_one_library(&ours, &[track(1, "One", "A", "Am")], &[], "2026-10-19").expect("write");
        let theirs = open_one_library(Path::new(&reference)).expect("open reference");
        let mine = open_one_library(&ours).expect("open ours");
        let schema = |c: &Connection| -> Vec<String> {
            let mut s = c.prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name").expect("schema");
            s.query_map([], |r| r.get::<_, String>(0))
                .expect("rows")
                .map(|r| r.expect("sql").split_whitespace().collect::<Vec<_>>().join(" "))
                .collect()
        };
        assert_eq!(schema(&mine), schema(&theirs), "schema");
        for table in ["menuItem", "category", "sort", "color"] {
            let rows = |c: &Connection| -> Vec<String> {
                let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY 1")).expect("rows");
                let n = s.column_count();
                s.query_map([], |r| {
                    Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).expect("cell"))).collect::<Vec<_>>().join("|"))
                })
                .expect("query")
                .map(|r| r.expect("row"))
                .collect()
            };
            assert_eq!(rows(&mine), rows(&theirs), "{table}");
        }
        let parents = |c: &Connection| -> Vec<(i64, i64, String)> {
            let mut s = c
                .prepare("SELECT myTag_id, sequenceNo, name FROM myTag WHERE myTag_id_parent = 0 ORDER BY myTag_id")
                .expect("myTag");
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).expect("rows").map(|r| r.expect("row")).collect()
        };
        assert_eq!(parents(&mine), parents(&theirs), "My Tag parents");
    }
}
