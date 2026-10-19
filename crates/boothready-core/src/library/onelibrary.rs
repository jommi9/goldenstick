//! Read rekordbox OneLibrary databases (`PIONEER/rekordbox/exportLibrary.db`).
//!
//! OneLibrary, called Device Library Plus before rekordbox 7.2, is a
//! SQLCipher 4 database with SQLCipher's default settings and one passphrase
//! shared by every copy. The passphrase is carried here the way djbooth
//! (`packages/rekordbox-ts/src/devicelib-plus/database.ts`, MIT) and
//! pyrekordbox (MIT) carry it: obfuscated as base85 text that decodes,
//! after an XOR with a fixed string and zlib decompression, to the key. See
//! `docs/ONELIBRARY-WRITER.md` for the format notes.

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::io::Read;
use std::path::Path;

/// djbooth's `DEVICE_LIBRARY_PLUS_KEY_BLOB`.
const KEY_BLOB: &str = "PN_1dH8$oLJY)16j_RvM6qphWw`476>;C1cWmI#se(PG`j}~xAjlufj?`#0i{;=glh(SkW)y0>n?YEiD`l%t(";
const BLOB_XOR: &[u8] = b"657f48f84c437cc1";
const BASE85: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
/// Every published OneLibrary key starts like this; djbooth and pyrekordbox
/// check it after decoding, and so does [`key`].
const KEY_PREFIX: &str = "r8gd";

#[derive(Debug, thiserror::Error)]
pub enum OneLibraryError {
    #[error("the OneLibrary key did not decode: {0}")]
    Key(String),
    #[error("{0}")]
    Sql(#[from] rusqlite::Error),
    #[error("the database failed its integrity check: {0}")]
    Integrity(String),
}

/// Decode the OneLibrary passphrase.
pub fn key() -> Result<String, OneLibraryError> {
    let decoded = decode_base85(KEY_BLOB.as_bytes())?;
    let xored: Vec<u8> = decoded.iter().enumerate().map(|(i, b)| b ^ BLOB_XOR[i % BLOB_XOR.len()]).collect();
    let mut text = String::new();
    flate2::read::ZlibDecoder::new(&xored[..])
        .read_to_string(&mut text)
        .map_err(|e| OneLibraryError::Key(e.to_string()))?;
    if !text.starts_with(KEY_PREFIX) {
        return Err(OneLibraryError::Key("unexpected prefix".into()));
    }
    Ok(text)
}

fn decode_base85(data: &[u8]) -> Result<Vec<u8>, OneLibraryError> {
    let padding = (5 - data.len() % 5) % 5;
    let mut padded = data.to_vec();
    padded.extend(std::iter::repeat_n(b'~', padding));
    let mut out = Vec::with_capacity(padded.len() / 5 * 4);
    for chunk in padded.chunks(5) {
        let mut value: u64 = 0;
        for &c in chunk {
            let digit = BASE85.iter().position(|&a| a == c).ok_or_else(|| OneLibraryError::Key("bad base85".into()))?;
            value = value * 85 + digit as u64;
        }
        let value = u32::try_from(value).map_err(|_| OneLibraryError::Key("base85 chunk over 32 bits".into()))?;
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.truncate(out.len() - padding);
    Ok(out)
}

/// Apply the OneLibrary key to a fresh connection. The pragma takes a
/// string literal, so the key is quoted; it holds no quote characters, but
/// they are escaped anyway.
pub(crate) fn unlock(conn: &Connection) -> Result<(), OneLibraryError> {
    let key = key()?;
    conn.execute_batch(&format!("PRAGMA key = '{}';", key.replace('\'', "''")))?;
    Ok(())
}

/// Open a OneLibrary database read-only with the shared key, and check that
/// the key fits by reading its schema.
pub fn open_one_library(path: &Path) -> Result<Connection, OneLibraryError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    unlock(&conn)?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
    Ok(conn)
}

/// One `content` row, in the shape [`crate::library::pdb::PdbTrack`] uses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OneLibraryTrack {
    pub id: u32,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub key: Option<String>,
    /// Volume-absolute path, e.g. `/Contents/Artist/x.mp3`.
    pub file_path: String,
    pub file_name: String,
    pub file_size: u64,
    pub file_type: u32,
    /// BPM × 100.
    pub tempo: u32,
    pub duration_secs: u32,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub analyze_path: String,
}

/// One `playlist` row with its tracks in playing order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OneLibraryPlaylist {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: u32,
    pub track_ids: Vec<u32>,
}

/// What a OneLibrary database holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OneLibrary {
    pub tracks: Vec<OneLibraryTrack>,
    pub playlists: Vec<OneLibraryPlaylist>,
    pub db_version: Option<String>,
}

/// Read tracks and playlists, after `PRAGMA integrity_check`.
pub fn read_one_library(conn: &Connection) -> Result<OneLibrary, OneLibraryError> {
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(OneLibraryError::Integrity(check));
    }
    let mut stmt = conn.prepare(
        "SELECT c.content_id, c.title, a.name, al.name, g.name, k.name, c.path, c.fileName,
                c.fileSize, c.fileType, c.bpmx100, c.length, c.bitrate, c.samplingRate, c.analysisDataFilePath
         FROM content c
         LEFT JOIN artist a ON a.artist_id = c.artist_id_artist
         LEFT JOIN album al ON al.album_id = c.album_id
         LEFT JOIN genre g ON g.genre_id = c.genre_id
         LEFT JOIN key k ON k.key_id = c.key_id
         ORDER BY c.content_id",
    )?;
    let tracks = stmt
        .query_map([], |r| {
            Ok(OneLibraryTrack {
                id: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                artist: r.get(2)?,
                album: r.get(3)?,
                genre: r.get(4)?,
                key: r.get(5)?,
                file_path: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                file_name: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                file_size: r.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0) as u64,
                file_type: r.get::<_, Option<u32>>(9)?.unwrap_or(0),
                tempo: r.get::<_, Option<u32>>(10)?.unwrap_or(0),
                duration_secs: r.get::<_, Option<u32>>(11)?.unwrap_or(0),
                bitrate: r.get::<_, Option<u32>>(12)?.unwrap_or(0),
                sample_rate: r.get::<_, Option<u32>>(13)?.unwrap_or(0),
                analyze_path: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare(
        "SELECT playlist_id, playlist_id_parent, name, attribute, sequenceNo FROM playlist
         ORDER BY playlist_id_parent, sequenceNo, playlist_id",
    )?;
    let mut playlists = stmt
        .query_map([], |r| {
            Ok(OneLibraryPlaylist {
                id: r.get(0)?,
                parent_id: r.get::<_, Option<u32>>(1)?.unwrap_or(0),
                name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                is_folder: r.get::<_, Option<u32>>(3)?.unwrap_or(0) == 1,
                sort_order: r.get::<_, Option<u32>>(4)?.unwrap_or(0),
                track_ids: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut members =
        conn.prepare("SELECT content_id FROM playlist_content WHERE playlist_id = ? ORDER BY sequenceNo")?;
    for p in &mut playlists {
        p.track_ids = members.query_map([p.id], |r| r.get(0))?.collect::<Result<Vec<u32>, _>>()?;
    }
    let db_version = conn.query_row("SELECT dbVersion FROM property LIMIT 1", [], |r| r.get(0)).optional()?;
    Ok(OneLibrary { tracks, playlists, db_version })
}

/// Open and read in one step.
pub fn read_one_library_file(path: &Path) -> Result<OneLibrary, OneLibraryError> {
    read_one_library(&open_one_library(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_decodes_to_the_published_prefix() {
        let k = key().expect("key");
        assert!(k.starts_with(KEY_PREFIX));
        assert_eq!(k.len(), 64);
    }

    #[test]
    fn base85_round_trips_a_known_value() {
        // "0" * 5 is zero; padding drops back to the original length.
        assert_eq!(decode_base85(b"00000").expect("decode"), vec![0, 0, 0, 0]);
        assert_eq!(decode_base85(b"000").expect("decode"), vec![0, 0]);
        assert!(decode_base85(b"0000\"").is_err());
    }
}
