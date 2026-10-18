//! Writer for rekordbox Device Library databases (`export.pdb`).
//!
//! This is a Rust port of djbooth's `LegacyDevicePdbWriter`
//! (`packages/rekordbox-ts/src/devicelib/pdb.ts`, `template.ts` and
//! `anlz-path.ts` in gakonst/djbooth at commit
//! `452c46f03c99e87df1793558dab1f69fc0a810a1`, MIT). The table layouts, string
//! encodings, page bookkeeping and ID allocation follow that source; see
//! `crates/boothready-core/UPSTREAM.md` for the differences.
//!
//! The writer starts from rekordbox's own empty export, which already holds
//! the twenty DeviceSQL tables and the colour, menu and column rows players
//! expect, and appends track, artist, album, genre, key, label, playlist and
//! history rows to it. Rows are appended to the last data page of their
//! table; a new page is taken from the file's free list when the current one
//! is full. The reader in [`super::pdb`] is the oracle every build is checked
//! against.

use super::pdb::{
    TABLE_ALBUMS, TABLE_ARTISTS, TABLE_GENRES, TABLE_KEYS, TABLE_PLAYLIST_ENTRIES, TABLE_PLAYLIST_TREE, TABLE_TRACKS,
};
use base64::Engine;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;

pub const TABLE_LABELS: u32 = 4;
pub const TABLE_HISTORY_PLAYLISTS: u32 = 11;
pub const TABLE_HISTORY_ENTRIES: u32 = 12;

const PAGE_HEADER_BYTES: usize = 0x28;
const TRACK_ROW_FIXED_BYTES: usize = 0x5E;
const TRACK_STRING_COUNT: usize = 21;
const TRACK_ROW_HEADER_BYTES: usize = TRACK_ROW_FIXED_BYTES + 2 * TRACK_STRING_COUNT;
const EMPTY_EXPORT_PDB_BYTES: usize = 167_936;
/// Rows per data page; see `append_row`.
const MAX_ROWS_PER_PAGE: usize = 255;

/// rekordbox's empty Device Library, gzip-compressed and base64-encoded, the
/// same way djbooth embeds it. The file is rekordcrate's
/// `data/complete_export/empty/PIONEER/rekordbox/export.pdb` (MPL-2.0, used
/// unmodified; SHA-256
/// `aa22564888963e1ad1771b20cc858a3dc36b235a4463309da244d6894d605a09`).
const EMPTY_EXPORT_PDB_GZIP_BASE64: &str =
    "H4sIAAAAAAACA+3du49cZxnA4Xdm7/e1d71rrx17ba/tTWKjxRUFEr5fiG+yHSmWKBwtK4KwjLGwoKAwd3PVcKegCFQoSkFJ\
    QUFFTZE/IyUFwsXwfme+tQPSWNCN4fltnj2Zc86cc2ac5uw7WUdk8xGLufhYGkkT8bx2an3EcBqqyrbR+pyRum48jVVl38l6\
    vIm6z3SaqspzX0szabY+Jy8l5qpyrJ1pR1WOUa5zoSrHXkq7qnLM3Wm5KudaSXuqcp59aW9Vzrk/vVKVc66mA1W5ltfTwXSo\
    XsPxdDit1Ws7kY6ko/WaXk3H0npIkiRJkjRYtf7tXr/22fKte6B74MOh3vputztU9DZ3D/y9CwAAAAwaP+mQJEmSJOn/tzLQ\
    3/5sf7/5f9nH/B8AAADM/yVJkiRJ0uBWfi/f9u/y6zf/L/uY/wMAAID5vyRJkiRJGtzK37u3/Xf39Zv/l33M/wEAAMD8X5Ik\
    SZIkDW4T0fvd/5PRf/5f9jH/BwAAAPN/SZIkSZI0uE1F7/f7T0f/+X/Zx/wfAAAAzP8lSZIkSdLgNpNG02z0n//P1O3m/wAA\
    APByzv9n6/3/a6ld141Ha+3S3NeavxNwu1YrYurG5+9/oT5u584TN7eanxMMDUXMXX/49v3PbTXbhofz8Z2te/e++JXm8chI\
    xMzFh1tb93vPHc0TTp3+0qO367HGxvLxmXuPtrbPnyeeu/Ho4YN7W35AI0mSJEnSf9G9+ExciU/G8dgX82WCn1+luXL/neZf\
    MP+fC///PwAAAPj//yVJkiRJ0iC3I6L5nP/O6D//L/uY/wMAAID5vyRJkiRJGtwW0kRajP7z/7KP+T8AAACY/0uSJEmSpMFt\
    V5pMS9F//l/2Mf8HAAAA839JkiRJkjS4LaeptDv6z//LPub/AAAAYP4vSZIkSZIGtz1pOq1E//l/2cf8HwAAAMz/JUmSJEnS\
    4LY3zaR90X/+X/Yx/wcAAADzf0mSJEmSNLi9kmbT/ug//y/7mP8DAACA+b8kSZIkSRrcyg8G5tJq9J//l33M/wEAAMD8X5Ik\
    SZIkDW4H03w6FP3n/wfrdvN/AAAAeDnn/4fq/f/rqd7cx967Q2t/mf6gvbc+bsXj6OyM+Ef3YpyPa3Ezvz/tRrTj69FZLOtP\
    57rbcTlu5fen3aH4Rt3/dFyJM/FmXG32H45v1vW3c//TcTbeaNaPxLejM1vWn4kbdd/R+E49dtmzHPtaXMwtY/Hd6MyX9Xfy\
    Ksp5n3bH40l0lnr7ns/nX463cnmzOc5EfK+e80rufSbXX2nWT8b3o3O0rL+ee17OY5czlOtdjX99NRFT8YN6fW/k8+8066bj\
    R3Xd2Xx9vfdjJn5cz3U2rueRrtdrmI2f1msuR73a7D0XP6vXfCbX9d6P2/U48/Hz6KyUbRdy25Vcu9pc29W6fUd8Kzq7y/Yb\
    UV7Vnfy+fbU749fRWS3bLuX5b+cze9e3mq/9dB6lvOML8Yt67kv1eeVKe69rMX5S3/db9f09G5dyy674ZT1neW1Xm2u5ls+8\
    lduW4ofR2Ve2nauvoryH5/LrfHraXY5fRedQb/unc9v2VZdru55Xd62+z7ujU899oXn/ztU/xT3x23q9Zc2FfO6bubX3nJX4\
    Xb2u8md3I6+196dctu6N9+q2q811lVey/d+Rn/xJkiRJern6Y/s37cftd9qn2qvt4faHrb+23m91Wt2x7tiD1u3WqdZ6a7EV\
    rQ/iT/Fu3qN9Oe7mHdBG3mc1s/zmHuhwlPvJiLUXzP8P1+3m/wAAAPByzv/X6v3/8Sjz+V5Lf26v/W32D7H0bP7fis1WmdOP\
    xkirzObHmsdjMd4sx2OiWU7EZLOcjKlmORNzzXI2FmK4VX7P4GKMtsp8e1ezfmcsN8t289WKoearndcx3DyaiunYzH/fkWfe\
    TAt5RZt59sV85maefW+sxGZ7PHbnHpt59qVcs5lnlyRJkiTpf7Xfx7vlk9/xJB7Hp/Lrq/Eg3om78VbzmehT8YnYiPVYzXvl\
    +bxffz7/PxLlPjzi6Avm/0fqdvN/AAAAeDnn/0fr/f+JKPP9XjtW22tH5540nwvozf9Hm58N7IqxZjkb482yzP3Lssz9y7LM\
    /ctyLmaaZZn/l+VyzDfLpTxiWe7pHTRWohn+N9P/8ssHyvS/fAhhpPetfNqgfGs+B5CnliRJkiRJz3rcfF7/P53/H0sLaf0F\
    8/9jdbv5PwAAALyc8//1ev//6kfu/1ursfbe3Pqzx4/bz/dfPrlx8uSJjfLPnv1TH9/Y2BjyIxdJkiRJkga08hmBfwI8oDkG\
    AJACAA==";

/// rekordbox `FileType` values for the audio formats a legacy Device Library
/// can hold, keyed by lower-case extension.
const FILE_TYPE_BY_EXTENSION: &[(&str, u16)] =
    &[("aif", 12), ("aiff", 12), ("flac", 5), ("m4a", 4), ("mp3", 1), ("wav", 11)];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WriterError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    Capacity(String),
    #[error("the Device Library writer is already finalized")]
    Finalized,
    #[error("the embedded empty export.pdb is damaged: {0}")]
    Template(String),
    #[error("the existing Device Library is damaged: {0}")]
    Damaged(String),
}

type Result<T> = std::result::Result<T, WriterError>;

fn invalid(msg: impl Into<String>) -> WriterError {
    WriterError::InvalidInput(msg.into())
}

fn damaged(msg: impl Into<String>) -> WriterError {
    WriterError::Damaged(msg.into())
}

/// Metadata written into one Device Library track row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInput {
    /// Stable positive DeviceSQL ID. `None` allocates the next free one.
    pub id: Option<u32>,
    pub title: String,
    /// Volume-absolute path, e.g. `/Contents/00000001/track.mp3`.
    pub file_path: String,
    /// Defaults to the last component of `file_path`.
    pub filename: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub key: Option<String>,
    pub label: Option<String>,
    pub comment: String,
    pub mix_name: String,
    /// `YYYY-MM-DD` or empty.
    pub release_date: String,
    /// Volume-absolute path of the track's `ANLZ0000.DAT`, or empty.
    pub analyze_path: String,
    /// `YYYY-MM-DD` or empty.
    pub date_added: String,
    /// BPM times 100.
    pub tempo: u32,
    /// Whole seconds.
    pub duration: u16,
    pub year: u16,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub sample_depth: u16,
    pub file_size: u32,
    pub track_number: u32,
    pub disc_number: u16,
    pub play_count: u16,
    pub rating: u8,
    pub color_id: u8,
    pub artwork_id: u32,
    /// rekordbox `FileType`; inferred from the extension when `None`.
    pub file_type: Option<u16>,
    /// Desktop master-library content ID retained by rekordbox's export.
    pub master_content_id: Option<u32>,
    /// Desktop master database ID, split across two 16-bit fields in the row.
    pub master_db_id: Option<u32>,
    /// Analysis revision; defaults to rekordbox's analyzed-row value.
    pub analysis_data_update_count: Option<u32>,
    /// Metadata revision; defaults to rekordbox's analyzed-row value.
    pub information_update_count: Option<u32>,
}

impl Default for TrackInput {
    fn default() -> Self {
        TrackInput {
            id: None,
            title: String::new(),
            file_path: String::new(),
            filename: None,
            artist: None,
            album: None,
            genre: None,
            key: None,
            label: None,
            comment: String::new(),
            mix_name: String::new(),
            release_date: String::new(),
            analyze_path: String::new(),
            date_added: String::new(),
            tempo: 0,
            duration: 0,
            year: 0,
            bitrate: 0,
            sample_rate: 44_100,
            sample_depth: 16,
            file_size: 0,
            track_number: 0,
            disc_number: 0,
            play_count: 0,
            rating: 0,
            color_id: 0,
            artwork_id: 0,
            file_type: None,
            master_content_id: None,
            master_db_id: None,
            analysis_data_update_count: None,
            information_update_count: None,
        }
    }
}

/// Track fields read back by [`inspect_pdb`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedTrack {
    pub id: u32,
    pub title: String,
    pub filename: String,
    pub file_path: String,
    pub analyze_path: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub key: Option<String>,
    pub tempo: u32,
    pub duration: u16,
    pub play_count: u16,
}

/// Playlist fields read back by [`inspect_pdb`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedPlaylist {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub folder: bool,
    pub track_ids: Vec<u32>,
}

/// One CDJ-created play history and its ordered track IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedHistory {
    pub id: u32,
    pub name: String,
    pub track_ids: Vec<u32>,
}

/// Bounded semantic inspection of a Device Library, the port of djbooth's
/// `inspectLegacyDevicePdb`. BoothReady's own reader in [`super::pdb`] is the
/// product's view of the file; this one also reads albums, genres, keys and
/// histories, which the writer needs when it appends to an existing export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdbInspection {
    pub page_size: u32,
    pub tracks: Vec<InspectedTrack>,
    pub playlists: Vec<InspectedPlaylist>,
    pub histories: Vec<InspectedHistory>,
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

/// Decompress rekordbox's empty Device Library.
pub fn empty_device_pdb() -> Result<Vec<u8>> {
    let gz = base64::engine::general_purpose::STANDARD
        .decode(EMPTY_EXPORT_PDB_GZIP_BASE64)
        .map_err(|e| WriterError::Template(format!("base64: {e}")))?;
    let mut data = Vec::with_capacity(EMPTY_EXPORT_PDB_BYTES);
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut data)
        .map_err(|e| WriterError::Template(format!("gzip: {e}")))?;
    if data.len() != EMPTY_EXPORT_PDB_BYTES {
        return Err(WriterError::Template(format!(
            "{} bytes after decompression, expected {EMPTY_EXPORT_PDB_BYTES}",
            data.len()
        )));
    }
    Ok(data)
}

/// Encode a DeviceSQL string the way rekordbox does: a one-byte header for
/// ASCII up to 126 bytes, a four-byte `0x40` header for longer ASCII, and
/// UTF-16LE behind a `0x90` header for anything else.
pub fn encode_device_sql_string(text: &str) -> Result<Vec<u8>> {
    if text.is_ascii() {
        let bytes = text.as_bytes();
        if bytes.len() <= 126 {
            let mut out = Vec::with_capacity(bytes.len() + 1);
            out.push(((bytes.len() + 1) * 2 + 1) as u8);
            out.extend_from_slice(bytes);
            return Ok(out);
        }
        if bytes.len() > 32_763 {
            return Err(invalid("DeviceSQL string is too long"));
        }
        let mut out = Vec::with_capacity(bytes.len() + 4);
        out.push(0x40);
        out.extend_from_slice(&((bytes.len() + 4) as u16).to_le_bytes());
        out.push(0);
        out.extend_from_slice(bytes);
        return Ok(out);
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let byte_len = units.len() * 2;
    if byte_len + 4 > 0x7fff {
        return Err(invalid("DeviceSQL string is too long"));
    }
    let mut out = Vec::with_capacity(byte_len + 4);
    out.push(0x90);
    out.extend_from_slice(&((byte_len + 4) as u16).to_le_bytes());
    out.push(0);
    for unit in units {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(out)
}

/// Decode a DeviceSQL string at `offset`, refusing anything malformed.
pub fn decode_device_sql_string(data: &[u8], offset: usize) -> Result<String> {
    let kind = *data.get(offset).ok_or_else(|| damaged(format!("string offset {offset} is outside the database")))?;
    if kind & 1 == 1 {
        let length = (usize::from(kind >> 1)).saturating_sub(1);
        let text = data
            .get(offset + 1..offset + 1 + length)
            .ok_or_else(|| damaged(format!("short string at {offset} runs past the end")))?;
        return Ok(String::from_utf8_lossy(text).into_owned());
    }
    let length = usize::from(u16_at(data, offset + 1).ok_or_else(|| damaged("truncated DeviceSQL string header"))?);
    if length < 4 || offset + length > data.len() {
        return Err(damaged("invalid DeviceSQL string length"));
    }
    let text = &data[offset + 4..offset + length];
    match kind {
        0x40 => Ok(String::from_utf8_lossy(text).into_owned()),
        0x90 => {
            let units: Vec<u16> = text.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Ok(String::from_utf16_lossy(&units))
        }
        other => Err(damaged(format!("unknown DeviceSQL string kind {other:#04x}"))),
    }
}

/// rekordbox `FileType` for an audio path, from its extension.
pub fn file_type_for_path(path: &str) -> Option<u16> {
    let ext = path.rsplit_once('.').map(|(_, e)| e)?;
    if ext.contains('/') {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    FILE_TYPE_BY_EXTENSION.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t)
}

fn inferred_file_type(path: &str) -> Result<u16> {
    file_type_for_path(path).ok_or_else(|| {
        let ext = path.rsplit_once('.').map(|(_, e)| format!(".{e}")).unwrap_or_else(|| "(no extension)".into());
        invalid(format!("Device Library export does not support '{ext}' audio"))
    })
}

/// Where rekordbox puts a track's analysis files on a USB, derived from the
/// volume-absolute audio path: `/PIONEER/USBANLZ/P<bucket>/<hash>/ANLZ0000.DAT`.
pub fn usb_analysis_path(usb_audio_path: &str) -> Result<String> {
    if !usb_audio_path.starts_with("/Contents/") {
        return Err(invalid(format!("USB audio path must start with '/Contents/': '{usb_audio_path}'")));
    }
    let mut hash: u32 = 0;
    for unit in usb_audio_path.encode_utf16() {
        let unit = u32::from(unit);
        let intermediate = hash.wrapping_mul(0x5bc9).wrapping_add(unit);
        hash = intermediate.wrapping_mul(0x93b5).wrapping_add(unit);
    }
    let path_hash = hash % 200_003;
    let mut bucket = 0u32;
    bucket |= path_hash & 0x01;
    bucket |= (path_hash >> 1) & 0x02;
    bucket |= (path_hash >> 4) & 0x04;
    bucket |= (path_hash >> 4) & 0x08;
    bucket |= (path_hash >> 5) & 0x10;
    bucket |= (path_hash >> 8) & 0x20;
    bucket |= (path_hash >> 10) & 0x40;
    Ok(format!("/PIONEER/USBANLZ/P{bucket:03X}/{path_hash:08X}/ANLZ0000.DAT"))
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u16(b: &[u8], o: usize) -> Result<u16> {
    u16_at(b, o).ok_or_else(|| damaged(format!("read past the end at byte {o}")))
}

fn read_u32(b: &[u8], o: usize) -> Result<u32> {
    u32_at(b, o).ok_or_else(|| damaged(format!("read past the end at byte {o}")))
}

fn read_u8(b: &[u8], o: usize) -> Result<u8> {
    b.get(o).copied().ok_or_else(|| damaged(format!("read past the end at byte {o}")))
}

fn directory_bytes(slots: usize) -> usize {
    if slots == 0 {
        0
    } else {
        2 * slots + 4 * slots.div_ceil(16)
    }
}

/// Absolute byte offsets of every present row in `table`.
fn table_rows(data: &[u8], table: u32) -> Result<Vec<usize>> {
    if data.len() < 4096 {
        return Err(damaged("truncated DeviceSQL database"));
    }
    let page_size = read_u32(data, 4)? as usize;
    if page_size == 0 {
        return Err(damaged("zero page size"));
    }
    let tables = read_u32(data, 8)? as usize;
    let mut first_last = None;
    for index in 0..tables {
        let offset = 0x1c + index * 16;
        if read_u32(data, offset)? == table {
            first_last = Some((read_u32(data, offset + 8)?, read_u32(data, offset + 12)?));
            break;
        }
    }
    let Some((first, last)) = first_last else { return Ok(vec![]) };
    let mut rows = Vec::new();
    let mut visited = HashSet::new();
    let mut page = first;
    loop {
        if !visited.insert(page) {
            return Err(damaged(format!("table {table} contains a page cycle")));
        }
        let page_offset = page as usize * page_size;
        if page_offset + page_size > data.len() {
            return Err(damaged(format!("page {page} is outside the database")));
        }
        if read_u8(data, page_offset + 0x1b)? & 0x40 == 0 {
            let slots = slot_count(data, page_offset)?;
            for group in 0..slots.div_ceil(16) {
                let base = page_offset + page_size - group * 0x24;
                let presence = read_u16(data, base - 4)?;
                let count = (slots - group * 16).min(16);
                for bit in 0..count {
                    if presence & (1 << bit) != 0 {
                        rows.push(page_offset + PAGE_HEADER_BYTES + usize::from(read_u16(data, base - 6 - bit * 2)?));
                    }
                }
            }
        }
        if page == last {
            break;
        }
        page = read_u32(data, page_offset + 0x0c)?;
    }
    Ok(rows)
}

fn slot_count(data: &[u8], page_offset: usize) -> Result<usize> {
    Ok(usize::from(read_u8(data, page_offset + 0x18)?) + 0x100 * usize::from(read_u8(data, page_offset + 0x19)? & 1))
}

/// Parse enough of a Device Library to verify what a writer produced, or to
/// seed a writer that appends to it.
pub fn inspect_pdb(data: &[u8]) -> Result<PdbInspection> {
    let page_size = read_u32(data, 4)?;
    let mut artists = HashMap::new();
    for offset in table_rows(data, TABLE_ARTISTS)? {
        let subtype = read_u16(data, offset)?;
        let name_offset = if subtype == 0x64 {
            usize::from(read_u16(data, offset + 0x0a)?)
        } else {
            usize::from(read_u8(data, offset + 9)?)
        };
        artists.insert(read_u32(data, offset + 4)?, decode_device_sql_string(data, offset + name_offset)?);
    }
    let mut albums = HashMap::new();
    for offset in table_rows(data, TABLE_ALBUMS)? {
        let name_offset = usize::from(read_u8(data, offset + 0x15)?);
        albums.insert(read_u32(data, offset + 12)?, decode_device_sql_string(data, offset + name_offset)?);
    }
    let mut genres = HashMap::new();
    for offset in table_rows(data, TABLE_GENRES)? {
        genres.insert(read_u32(data, offset)?, decode_device_sql_string(data, offset + 4)?);
    }
    let mut keys = HashMap::new();
    for offset in table_rows(data, TABLE_KEYS)? {
        keys.insert(read_u32(data, offset)?, decode_device_sql_string(data, offset + 8)?);
    }
    let mut tracks = Vec::new();
    for offset in table_rows(data, TABLE_TRACKS)? {
        let string = |index: usize| -> Result<String> {
            let rel = usize::from(read_u16(data, offset + TRACK_ROW_FIXED_BYTES + index * 2)?);
            decode_device_sql_string(data, offset + rel)
        };
        tracks.push(InspectedTrack {
            id: read_u32(data, offset + 0x48)?,
            title: string(17)?,
            filename: string(19)?,
            file_path: string(20)?,
            analyze_path: string(14)?,
            artist: artists.get(&read_u32(data, offset + 0x44)?).cloned(),
            album: albums.get(&read_u32(data, offset + 0x40)?).cloned(),
            genre: genres.get(&read_u32(data, offset + 0x3c)?).cloned(),
            key: keys.get(&read_u32(data, offset + 0x20)?).cloned(),
            tempo: read_u32(data, offset + 0x38)?,
            duration: read_u16(data, offset + 0x54)?,
            play_count: read_u16(data, offset + 0x4e)?,
        });
    }
    let mut memberships: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for offset in table_rows(data, TABLE_PLAYLIST_ENTRIES)? {
        memberships
            .entry(read_u32(data, offset + 8)?)
            .or_default()
            .push((read_u32(data, offset)?, read_u32(data, offset + 4)?));
    }
    let mut playlists = Vec::new();
    for offset in table_rows(data, TABLE_PLAYLIST_TREE)? {
        let id = read_u32(data, offset + 12)?;
        let mut entries = memberships.remove(&id).unwrap_or_default();
        entries.sort_by_key(|(index, _)| *index);
        playlists.push(InspectedPlaylist {
            id,
            parent_id: read_u32(data, offset)?,
            name: decode_device_sql_string(data, offset + 20)?,
            folder: read_u32(data, offset + 16)? != 0,
            track_ids: entries.into_iter().map(|(_, track)| track).collect(),
        });
    }
    let mut history_memberships: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for offset in table_rows(data, TABLE_HISTORY_ENTRIES)? {
        history_memberships
            .entry(read_u32(data, offset + 4)?)
            .or_default()
            .push((read_u32(data, offset + 8)?, read_u32(data, offset)?));
    }
    let mut histories = Vec::new();
    for offset in table_rows(data, TABLE_HISTORY_PLAYLISTS)? {
        let id = read_u32(data, offset)?;
        let mut entries = history_memberships.remove(&id).unwrap_or_default();
        entries.sort_by_key(|(index, _)| *index);
        histories.push(InspectedHistory {
            id,
            name: decode_device_sql_string(data, offset + 4)?,
            track_ids: entries.into_iter().map(|(_, track)| track).collect(),
        });
    }
    Ok(PdbInspection { page_size, tracks, playlists, histories })
}

/// Builds a `PIONEER/rekordbox/export.pdb` for CDJ-3000 class players.
///
/// Create one writer per export, add every row, then call [`build`](Self::build).
#[derive(Debug)]
pub struct DevicePdbWriter {
    data: Vec<u8>,
    page_size: usize,
    original_slots: HashMap<u32, usize>,
    appended_slots: HashMap<u32, usize>,
    new_generations: HashMap<u32, u32>,
    touched_pages: BTreeMap<u32, BTreeSet<u32>>,
    artist_ids: HashMap<String, u32>,
    album_ids: HashMap<(u32, String), u32>,
    genre_ids: HashMap<String, u32>,
    key_ids: HashMap<String, u32>,
    label_ids: HashMap<String, u32>,
    playlist_folders: HashMap<u32, bool>,
    playlist_sort_orders: HashMap<u32, u32>,
    playlist_entry_counts: HashMap<u32, u32>,
    track_ids: HashSet<u32>,
    history_ids: HashSet<u32>,
    history_entry_counts: HashMap<u32, u32>,
    track_count: usize,
    next_track_id: u32,
    playlist_count: u32,
    next_history_id: u32,
    finalized: bool,
}

fn next_entity_id<K>(ids: &HashMap<K, u32>) -> u32 {
    ids.values().copied().max().unwrap_or(0) + 1
}

impl DevicePdbWriter {
    /// Start from rekordbox's empty Device Library.
    pub fn new() -> Result<Self> {
        Self::from_bytes(empty_device_pdb()?, false)
    }

    /// Append to an existing Device Library. Its tracks, playlists, named
    /// rows and histories are read so new rows reuse their IDs.
    pub fn from_existing(existing: &[u8]) -> Result<Self> {
        Self::from_bytes(existing.to_vec(), true)
    }

    fn from_bytes(data: Vec<u8>, seed: bool) -> Result<Self> {
        let page_size = read_u32(&data, 4)? as usize;
        if page_size != 4096 {
            return Err(invalid(format!("unsupported DeviceSQL page size {page_size}")));
        }
        let mut writer = DevicePdbWriter {
            data,
            page_size,
            original_slots: HashMap::new(),
            appended_slots: HashMap::new(),
            new_generations: HashMap::new(),
            touched_pages: BTreeMap::new(),
            artist_ids: HashMap::new(),
            album_ids: HashMap::new(),
            genre_ids: HashMap::new(),
            key_ids: HashMap::new(),
            label_ids: HashMap::new(),
            playlist_folders: HashMap::new(),
            playlist_sort_orders: HashMap::new(),
            playlist_entry_counts: HashMap::new(),
            track_ids: HashSet::new(),
            history_ids: HashSet::new(),
            history_entry_counts: HashMap::new(),
            track_count: 0,
            next_track_id: 0,
            playlist_count: 0,
            next_history_id: 0,
            finalized: false,
        };
        if seed {
            writer.seed_existing_rows()?;
        }
        Ok(writer)
    }

    /// Tracks added so far, counting those read from an existing export.
    pub fn track_count(&self) -> usize {
        self.track_count
    }

    /// Append one track and return its DeviceSQL track ID.
    pub fn add_track(&mut self, input: &TrackInput) -> Result<u32> {
        self.require_mutable()?;
        if input.title.is_empty() {
            return Err(invalid("Device Library track title is required"));
        }
        if !input.file_path.starts_with('/') {
            return Err(invalid(format!("Device Library file path must be absolute: '{}'", input.file_path)));
        }
        let file_type = match input.file_type {
            Some(t) => t,
            None => inferred_file_type(&input.file_path)?,
        };
        let filename =
            input.filename.clone().unwrap_or_else(|| input.file_path.rsplit('/').next().unwrap_or("").to_string());
        let analysis_count = input.analysis_data_update_count.unwrap_or(2).to_string();
        let information_count = input.information_update_count.unwrap_or(2).to_string();
        let texts: [&str; TRACK_STRING_COUNT] = [
            "",
            "",
            &analysis_count,
            &information_count,
            "",
            "",
            "ON",
            "ON",
            "",
            "",
            &input.date_added,
            &input.release_date,
            &input.mix_name,
            "",
            &input.analyze_path,
            &input.date_added,
            &input.comment,
            &input.title,
            "",
            &filename,
            &input.file_path,
        ];
        let strings = texts.iter().map(|t| encode_device_sql_string(t)).collect::<Result<Vec<_>>>()?;
        let allocation = 0x88 + strings.iter().map(|s| align4(s.len())).sum::<usize>() + 4;
        self.require_row_capacity(allocation)?;

        let artist_id = match &input.artist {
            Some(a) if !a.is_empty() => self.get_or_create_artist(a)?,
            _ => 0,
        };
        let album_id = match &input.album {
            Some(a) if !a.is_empty() => self.get_or_create_album(a, artist_id)?,
            _ => 0,
        };
        let genre_id = match &input.genre {
            Some(g) if !g.is_empty() => self.get_or_create_named(TABLE_GENRES, g)?,
            _ => 0,
        };
        let key_id = match &input.key {
            Some(k) if !k.is_empty() => self.get_or_create_key(k)?,
            _ => 0,
        };
        let label_id = match &input.label {
            Some(l) if !l.is_empty() => self.get_or_create_named(TABLE_LABELS, l)?,
            _ => 0,
        };
        let track_id = match input.id {
            Some(id) => id,
            None => self.next_available_track_id(),
        };
        if track_id == 0 {
            return Err(invalid("Device Library track ID must be positive"));
        }
        if !self.track_ids.insert(track_id) {
            return Err(invalid(format!("Device Library track ID {track_id} already exists")));
        }
        self.track_count += 1;
        self.next_track_id = self.next_track_id.max(track_id);
        let master_content_id = input.master_content_id.unwrap_or(0x0010_0000 + track_id);
        let master_db_id = input.master_db_id.unwrap_or(0x03dd_ae49);

        let mut row = Vec::with_capacity(allocation);
        let put16 = |row: &mut Vec<u8>, v: u16| row.extend_from_slice(&v.to_le_bytes());
        let put32 = |row: &mut Vec<u8>, v: u32| row.extend_from_slice(&v.to_le_bytes());
        put16(&mut row, 0x0024);
        put16(&mut row, 0);
        put32(&mut row, 0x000c_0700);
        put32(&mut row, input.sample_rate);
        put32(&mut row, 0);
        put32(&mut row, input.file_size);
        put32(&mut row, master_content_id);
        put16(&mut row, (master_db_id & 0xffff) as u16);
        put16(&mut row, (master_db_id >> 16) as u16);
        put32(&mut row, input.artwork_id);
        put32(&mut row, key_id);
        put32(&mut row, 0);
        put32(&mut row, label_id);
        put32(&mut row, 0);
        put32(&mut row, input.bitrate);
        put32(&mut row, input.track_number);
        put32(&mut row, input.tempo);
        put32(&mut row, genre_id);
        put32(&mut row, album_id);
        put32(&mut row, artist_id);
        put32(&mut row, track_id);
        put16(&mut row, input.disc_number);
        put16(&mut row, input.play_count);
        put16(&mut row, input.year);
        put16(&mut row, input.sample_depth);
        put16(&mut row, input.duration);
        put16(&mut row, 0x0029);
        row.push(input.color_id);
        row.push(input.rating);
        put16(&mut row, file_type);
        put16(&mut row, 0x0003);
        debug_assert_eq!(row.len(), TRACK_ROW_FIXED_BYTES);

        let mut offsets = Vec::with_capacity(TRACK_STRING_COUNT * 2);
        let mut blobs: Vec<u8> = Vec::new();
        let mut position = TRACK_ROW_HEADER_BYTES;
        for encoded in &strings {
            if encoded[0] & 1 == 0 && position % 4 != 0 {
                let padding = 4 - position % 4;
                blobs.extend(std::iter::repeat_n(0u8, padding));
                position += padding;
            }
            offsets.extend_from_slice(&(position as u16).to_le_bytes());
            blobs.extend_from_slice(encoded);
            position += encoded.len();
        }
        row.extend_from_slice(&offsets);
        row.extend_from_slice(&blobs);
        self.append_row(TABLE_TRACKS, &row, allocation, Some(2))?;
        Ok(track_id)
    }

    /// Append a playlist or folder and return its node ID. `parent_id` 0 is the root.
    pub fn create_playlist(&mut self, name: &str, parent_id: u32, folder: bool) -> Result<u32> {
        self.require_mutable()?;
        if name.is_empty() {
            return Err(invalid("Device Library playlist name is required"));
        }
        if parent_id != 0 && self.playlist_folders.get(&parent_id) != Some(&true) {
            return Err(invalid(format!("playlist parent {parent_id} is missing or is not a folder")));
        }
        self.playlist_count += 1;
        let node_id = self.playlist_count;
        let sort_order = *self.playlist_sort_orders.get(&parent_id).unwrap_or(&0);
        self.playlist_sort_orders.insert(parent_id, sort_order + 1);
        let encoded = encode_device_sql_string(name)?;
        let mut row = Vec::with_capacity(20 + encoded.len());
        row.extend_from_slice(&parent_id.to_le_bytes());
        row.extend_from_slice(&0u32.to_le_bytes());
        row.extend_from_slice(&sort_order.to_le_bytes());
        row.extend_from_slice(&node_id.to_le_bytes());
        row.extend_from_slice(&u32::from(folder).to_le_bytes());
        row.extend_from_slice(&encoded);
        let allocation = align4(row.len());
        self.append_row(TABLE_PLAYLIST_TREE, &row, allocation, None)?;
        self.playlist_folders.insert(node_id, folder);
        Ok(node_id)
    }

    /// Append an ordered membership to a non-folder playlist. `entry_index`
    /// defaults to the next position, counting from 1.
    pub fn add_to_playlist(&mut self, playlist_id: u32, track_id: u32, entry_index: Option<u32>) -> Result<()> {
        self.require_mutable()?;
        if self.playlist_folders.get(&playlist_id) != Some(&false) {
            return Err(invalid(format!("playlist {playlist_id} is missing or is a folder")));
        }
        if !self.track_ids.contains(&track_id) {
            return Err(invalid(format!("track {track_id} does not exist")));
        }
        let current = *self.playlist_entry_counts.get(&playlist_id).unwrap_or(&0);
        let position = entry_index.unwrap_or(current + 1);
        let mut row = Vec::with_capacity(12);
        row.extend_from_slice(&position.to_le_bytes());
        row.extend_from_slice(&track_id.to_le_bytes());
        row.extend_from_slice(&playlist_id.to_le_bytes());
        self.append_row(TABLE_PLAYLIST_ENTRIES, &row, row.len(), None)?;
        self.playlist_entry_counts.insert(playlist_id, position.max(current));
        Ok(())
    }

    /// Append a CDJ-style history playlist and return its ID.
    pub fn create_history(&mut self, name: &str, id: Option<u32>) -> Result<u32> {
        self.require_mutable()?;
        if name.is_empty() {
            return Err(invalid("Device Library history name is required"));
        }
        let history_id = match id {
            Some(id) => id,
            None => self.next_available_history_id(),
        };
        if history_id == 0 {
            return Err(invalid("Device Library history ID must be positive"));
        }
        if self.history_ids.contains(&history_id) {
            return Err(invalid(format!("Device Library history ID {history_id} already exists")));
        }
        let encoded = encode_device_sql_string(name)?;
        let mut row = Vec::with_capacity(4 + encoded.len());
        row.extend_from_slice(&history_id.to_le_bytes());
        row.extend_from_slice(&encoded);
        let allocation = align4(row.len());
        self.append_row(TABLE_HISTORY_PLAYLISTS, &row, allocation, None)?;
        self.history_ids.insert(history_id);
        self.next_history_id = self.next_history_id.max(history_id);
        Ok(history_id)
    }

    /// Append one ordered track occurrence to a history playlist.
    pub fn add_to_history(&mut self, history_id: u32, track_id: u32, entry_index: Option<u32>) -> Result<()> {
        self.require_mutable()?;
        if !self.history_ids.contains(&history_id) {
            return Err(invalid(format!("history {history_id} does not exist")));
        }
        if !self.track_ids.contains(&track_id) {
            return Err(invalid(format!("track {track_id} does not exist")));
        }
        let current = *self.history_entry_counts.get(&history_id).unwrap_or(&0);
        let position = entry_index.unwrap_or(current + 1);
        let mut row = Vec::with_capacity(12);
        row.extend_from_slice(&track_id.to_le_bytes());
        row.extend_from_slice(&history_id.to_le_bytes());
        row.extend_from_slice(&position.to_le_bytes());
        self.append_row(TABLE_HISTORY_ENTRIES, &row, row.len(), None)?;
        self.history_entry_counts.insert(history_id, position.max(current));
        Ok(())
    }

    /// Remove every CDJ history row while keeping all unrelated bytes.
    pub fn clear_histories(&mut self) -> Result<()> {
        self.require_mutable()?;
        for table in [TABLE_HISTORY_PLAYLISTS, TABLE_HISTORY_ENTRIES] {
            self.clear_table_rows(table)?;
        }
        self.history_ids.clear();
        self.history_entry_counts.clear();
        self.next_history_id = 0;
        Ok(())
    }

    /// Stamp page generations and the file sequence, then return the database.
    /// Further rows cannot be added afterwards.
    pub fn build(&mut self) -> Result<Vec<u8>> {
        if !self.finalized {
            let touched = std::mem::take(&mut self.touched_pages);
            for (table, pages) in &touched {
                let generation = *self
                    .new_generations
                    .get(table)
                    .ok_or_else(|| damaged(format!("missing generation for table {table}")))?;
                for page in pages {
                    self.put_u32(*page as usize * self.page_size + 0x10, generation)?;
                }
            }
            if !touched.is_empty() {
                let sequence = read_u32(&self.data, 0x14)?;
                self.put_u32(0x14, sequence + 1)?;
            }
            self.touched_pages = touched;
            self.finalized = true;
        }
        Ok(self.data.clone())
    }

    fn require_mutable(&self) -> Result<()> {
        if self.finalized {
            return Err(WriterError::Finalized);
        }
        Ok(())
    }

    fn next_available_track_id(&mut self) -> u32 {
        loop {
            self.next_track_id += 1;
            if !self.track_ids.contains(&self.next_track_id) {
                return self.next_track_id;
            }
        }
    }

    fn next_available_history_id(&mut self) -> u32 {
        loop {
            self.next_history_id += 1;
            if !self.history_ids.contains(&self.next_history_id) {
                return self.next_history_id;
            }
        }
    }

    fn seed_existing_rows(&mut self) -> Result<()> {
        let inspection = inspect_pdb(&self.data)?;
        for track in &inspection.tracks {
            self.track_ids.insert(track.id);
        }
        self.track_count = inspection.tracks.len();
        self.next_track_id = inspection.tracks.iter().map(|t| t.id).max().unwrap_or(0);
        self.playlist_count = inspection.playlists.iter().map(|p| p.id).max().unwrap_or(0);
        for playlist in &inspection.playlists {
            self.playlist_folders.insert(playlist.id, playlist.folder);
            self.playlist_entry_counts.insert(playlist.id, playlist.track_ids.len() as u32);
        }
        for offset in table_rows(&self.data, TABLE_PLAYLIST_TREE)? {
            let parent_id = read_u32(&self.data, offset)?;
            let sort_order = read_u32(&self.data, offset + 8)?;
            let current = *self.playlist_sort_orders.get(&parent_id).unwrap_or(&0);
            self.playlist_sort_orders.insert(parent_id, current.max(sort_order + 1));
        }
        for history in &inspection.histories {
            self.history_ids.insert(history.id);
            self.history_entry_counts.insert(history.id, history.track_ids.len() as u32);
            self.next_history_id = self.next_history_id.max(history.id);
        }
        for offset in table_rows(&self.data, TABLE_ARTISTS)? {
            let subtype = read_u16(&self.data, offset)?;
            let name_offset = if subtype == 0x64 {
                usize::from(read_u16(&self.data, offset + 0x0a)?)
            } else {
                usize::from(read_u8(&self.data, offset + 9)?)
            };
            let name = decode_device_sql_string(&self.data, offset + name_offset)?;
            self.artist_ids.insert(name, read_u32(&self.data, offset + 4)?);
        }
        for offset in table_rows(&self.data, TABLE_ALBUMS)? {
            let artist_id = read_u32(&self.data, offset + 8)?;
            let id = read_u32(&self.data, offset + 12)?;
            let name_offset = usize::from(read_u8(&self.data, offset + 0x15)?);
            let name = decode_device_sql_string(&self.data, offset + name_offset)?;
            self.album_ids.insert((artist_id, name), id);
        }
        for offset in table_rows(&self.data, TABLE_GENRES)? {
            let name = decode_device_sql_string(&self.data, offset + 4)?;
            self.genre_ids.insert(name, read_u32(&self.data, offset)?);
        }
        for offset in table_rows(&self.data, TABLE_LABELS)? {
            let name = decode_device_sql_string(&self.data, offset + 4)?;
            self.label_ids.insert(name, read_u32(&self.data, offset)?);
        }
        for offset in table_rows(&self.data, TABLE_KEYS)? {
            let name = decode_device_sql_string(&self.data, offset + 8)?;
            self.key_ids.insert(name, read_u32(&self.data, offset)?);
        }
        Ok(())
    }

    fn get_or_create_artist(&mut self, name: &str) -> Result<u32> {
        if let Some(&id) = self.artist_ids.get(name) {
            return Ok(id);
        }
        let id = next_entity_id(&self.artist_ids);
        let encoded = encode_device_sql_string(name)?;
        let mut row = vec![0x60, 0, 0, 0];
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&[0x03, 0x0a]);
        row.extend_from_slice(&encoded);
        let allocation = align4(10 + align4(encoded.len())) + 4;
        self.append_row(TABLE_ARTISTS, &row, allocation, Some(2))?;
        self.artist_ids.insert(name.to_string(), id);
        Ok(id)
    }

    fn get_or_create_album(&mut self, name: &str, artist_id: u32) -> Result<u32> {
        let key = (artist_id, name.to_string());
        if let Some(&id) = self.album_ids.get(&key) {
            return Ok(id);
        }
        let id = next_entity_id(&self.album_ids);
        let encoded = encode_device_sql_string(name)?;
        let mut row = vec![0x80, 0, 0, 0];
        row.extend_from_slice(&0u32.to_le_bytes());
        row.extend_from_slice(&artist_id.to_le_bytes());
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&0u32.to_le_bytes());
        row.extend_from_slice(&[0x03, 0x16]);
        row.extend_from_slice(&encoded);
        let allocation = align4(22 + align4(encoded.len())) + 4;
        self.append_row(TABLE_ALBUMS, &row, allocation, Some(2))?;
        self.album_ids.insert(key, id);
        Ok(id)
    }

    /// Genres and labels share one row shape: the ID, then the name.
    fn get_or_create_named(&mut self, table: u32, name: &str) -> Result<u32> {
        let ids = if table == TABLE_GENRES { &self.genre_ids } else { &self.label_ids };
        if let Some(&id) = ids.get(name) {
            return Ok(id);
        }
        let id = next_entity_id(ids);
        let encoded = encode_device_sql_string(name)?;
        let mut row = id.to_le_bytes().to_vec();
        row.extend_from_slice(&encoded);
        let allocation = align4(row.len());
        self.append_row(table, &row, allocation, None)?;
        let ids = if table == TABLE_GENRES { &mut self.genre_ids } else { &mut self.label_ids };
        ids.insert(name.to_string(), id);
        Ok(id)
    }

    fn get_or_create_key(&mut self, name: &str) -> Result<u32> {
        if let Some(&id) = self.key_ids.get(name) {
            return Ok(id);
        }
        let id = next_entity_id(&self.key_ids);
        let encoded = encode_device_sql_string(name)?;
        let mut row = id.to_le_bytes().to_vec();
        row.extend_from_slice(&id.to_le_bytes());
        row.extend_from_slice(&encoded);
        let allocation = align4(row.len());
        self.append_row(TABLE_KEYS, &row, allocation, None)?;
        self.key_ids.insert(name.to_string(), id);
        Ok(id)
    }

    fn append_row(
        &mut self,
        table: u32,
        row: &[u8],
        allocation: usize,
        index_shift_offset: Option<usize>,
    ) -> Result<()> {
        self.require_row_capacity(allocation)?;
        let table_offset = self.table_entry_offset(table)?;
        let mut page_index = read_u32(&self.data, table_offset + 12)?;
        let mut page_offset = page_index as usize * self.page_size;
        if read_u8(&self.data, page_offset + 0x1b)? & 0x40 != 0 {
            page_index = self.allocate_page(table)?;
            page_offset = page_index as usize * self.page_size;
        }

        let mut slots = slot_count(&self.data, page_offset)?;
        let mut used = usize::from(read_u16(&self.data, page_offset + 0x1e)?);
        // djbooth fills a page up to 511 slots and stores the ninth bit of the
        // count in the byte at 0x19. Readers written from crate-digger's
        // notes, BoothReady's own among them, only take the byte at 0x18, so
        // a page is closed at 255 rows here; rekordbox itself only exceeds
        // that on 12-byte playlist entry pages.
        if slots >= MAX_ROWS_PER_PAGE
            || PAGE_HEADER_BYTES + used + allocation + directory_bytes(slots + 1) > self.page_size
        {
            page_index = self.allocate_page(table)?;
            page_offset = page_index as usize * self.page_size;
            slots = 0;
            used = 0;
        }

        if let std::collections::hash_map::Entry::Vacant(entry) = self.original_slots.entry(page_index) {
            entry.insert(slots);
            self.appended_slots.insert(page_index, 0);
            for group in 0..slots.div_ceil(16) {
                self.put_u16(page_offset + self.page_size - group * 0x24 - 2, 0)?;
            }
        }
        self.mark_table_touched(table, page_index)?;

        let group = slots / 16;
        let bit = slots % 16;
        let group_base = page_offset + self.page_size - group * 0x24;
        if bit == 0 {
            self.put_u16(group_base - 4, 0)?;
            self.put_u16(group_base - 2, 0)?;
        }

        let row_start = page_offset + PAGE_HEADER_BYTES + used;
        if row.len() > allocation || row_start + allocation > self.data.len() {
            return Err(WriterError::Capacity(format!(
                "row of {} bytes does not fit its {allocation}-byte allocation",
                row.len()
            )));
        }
        self.data[row_start..row_start + allocation].fill(0);
        self.data[row_start..row_start + row.len()].copy_from_slice(row);
        if let Some(shift) = index_shift_offset {
            self.put_u16(row_start + shift, (0x20 * slots) as u16)?;
        }

        self.put_u16(group_base - 6 - 2 * bit, used as u16)?;
        let presence = read_u16(&self.data, group_base - 4)? | (1 << bit);
        self.put_u16(group_base - 4, presence)?;
        let flags = read_u16(&self.data, group_base - 2)? | (1 << bit);
        self.put_u16(group_base - 2, flags)?;
        let appended = self.appended_slots.get(&page_index).copied().unwrap_or(0) + 1;
        self.appended_slots.insert(page_index, appended);

        let new_slots = slots + 1;
        if new_slots > 511 {
            return Err(WriterError::Capacity("DeviceSQL page exceeds its 511-slot limit".into()));
        }
        let present = self.present_count(page_offset, new_slots)?;
        self.data[page_offset + 0x18] = (new_slots & 0xff) as u8;
        self.put_u16(page_offset + 0x19, (0x20 * present as u16) | u16::from(new_slots > 255))?;
        used += allocation;
        self.put_u16(page_offset + 0x1e, used as u16)?;
        self.put_u16(
            page_offset + 0x1c,
            (self.page_size - PAGE_HEADER_BYTES - used - directory_bytes(new_slots)) as u16,
        )?;
        self.put_u16(page_offset + 0x20, appended as u16)?;
        let original = self.original_slots.get(&page_index).copied().unwrap_or(0);
        self.put_u16(page_offset + 0x22, original as u16)?;
        Ok(())
    }

    fn clear_table_rows(&mut self, table: u32) -> Result<()> {
        let table_offset = self.table_entry_offset(table)?;
        let mut page = read_u32(&self.data, table_offset + 8)?;
        let last = read_u32(&self.data, table_offset + 12)?;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(page) {
                return Err(damaged(format!("table {table} contains a page cycle")));
            }
            let page_offset = page as usize * self.page_size;
            if read_u8(&self.data, page_offset + 0x1b)? & 0x40 == 0 {
                let slots = slot_count(&self.data, page_offset)?;
                for group in 0..slots.div_ceil(16) {
                    let group_base = page_offset + self.page_size - group * 0x24;
                    self.put_u16(group_base - 4, 0)?;
                    self.put_u16(group_base - 2, 0)?;
                }
                self.put_u16(page_offset + 0x19, u16::from(slots > 255))?;
                self.put_u16(page_offset + 0x20, 0)?;
                self.put_u16(page_offset + 0x22, slots as u16)?;
                self.mark_table_touched(table, page)?;
            }
            if page == last {
                break;
            }
            page = read_u32(&self.data, page_offset + 0x0c)?;
        }
        Ok(())
    }

    /// Take the table's reserved empty page as its new last data page and
    /// reserve the next unused page of the file in its place.
    fn allocate_page(&mut self, table: u32) -> Result<u32> {
        let table_offset = self.table_entry_offset(table)?;
        let page_index = read_u32(&self.data, table_offset + 4)?;
        let next_unused = read_u32(&self.data, 12)?;
        self.ensure_length((page_index as usize + 1) * self.page_size);
        let previous_last = read_u32(&self.data, table_offset + 12)?;
        let page_offset = page_index as usize * self.page_size;
        self.data[page_offset..page_offset + self.page_size].fill(0);
        self.put_u32(page_offset + 4, page_index)?;
        self.put_u32(page_offset + 8, table)?;
        self.put_u32(page_offset + 12, next_unused)?;
        self.data[page_offset + 0x1b] = 0x24;
        self.put_u16(page_offset + 0x1c, (self.page_size - PAGE_HEADER_BYTES) as u16)?;
        self.put_u32(table_offset + 12, page_index)?;
        self.put_u32(table_offset + 4, next_unused)?;
        self.put_u32(12, next_unused + 1)?;

        let previous_offset = previous_last as usize * self.page_size;
        if read_u8(&self.data, previous_offset + 0x1b)? & 0x40 != 0 {
            let heap = previous_offset + PAGE_HEADER_BYTES;
            if read_u32(&self.data, heap + 4)? == 0x03ff_ffff {
                self.put_u32(heap + 4, page_index)?;
            }
        }
        self.original_slots.insert(page_index, 0);
        self.appended_slots.insert(page_index, 0);
        self.mark_table_touched(table, page_index)?;
        Ok(page_index)
    }

    fn mark_table_touched(&mut self, table: u32, page_index: u32) -> Result<()> {
        if !self.new_generations.contains_key(&table) {
            let table_offset = self.table_entry_offset(table)?;
            let mut page = read_u32(&self.data, table_offset + 8)?;
            let last = read_u32(&self.data, table_offset + 12)?;
            let mut generation = 0u32;
            let mut visited = HashSet::new();
            loop {
                if !visited.insert(page) {
                    return Err(damaged(format!("table {table} contains a page cycle")));
                }
                generation = generation.max(read_u32(&self.data, page as usize * self.page_size + 0x10)?);
                if page == last {
                    break;
                }
                page = read_u32(&self.data, page as usize * self.page_size + 0x0c)?;
            }
            self.new_generations.insert(table, generation + 1);
        }
        self.touched_pages.entry(table).or_default().insert(page_index);
        Ok(())
    }

    fn require_row_capacity(&self, allocation: usize) -> Result<()> {
        let maximum = self.page_size - PAGE_HEADER_BYTES - directory_bytes(1);
        if allocation < 1 || allocation > maximum {
            return Err(WriterError::Capacity(format!(
                "DeviceSQL row allocation {allocation} exceeds page capacity {maximum}"
            )));
        }
        Ok(())
    }

    fn table_entry_offset(&self, table: u32) -> Result<usize> {
        let count = read_u32(&self.data, 8)?;
        for index in 0..count as usize {
            let offset = 0x1c + index * 16;
            if read_u32(&self.data, offset)? == table {
                return Ok(offset);
            }
        }
        Err(damaged(format!("Device Library has no table {table}")))
    }

    fn present_count(&self, page_offset: usize, slots: usize) -> Result<usize> {
        let mut count = 0usize;
        for group in 0..slots.div_ceil(16) {
            let base = page_offset + self.page_size - group * 0x24;
            let in_group = (slots - group * 16).min(16);
            let mask: u16 = if in_group == 16 { 0xffff } else { (1u16 << in_group) - 1 };
            count += (read_u16(&self.data, base - 4)? & mask).count_ones() as usize;
        }
        Ok(count)
    }

    fn ensure_length(&mut self, length: usize) {
        if self.data.len() < length {
            self.data.resize(length, 0);
        }
    }

    fn put_u16(&mut self, offset: usize, value: u16) -> Result<()> {
        let slot = self
            .data
            .get_mut(offset..offset + 2)
            .ok_or_else(|| damaged(format!("write past the end at byte {offset}")))?;
        slot.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn put_u32(&mut self, offset: usize, value: u32) -> Result<()> {
        let slot = self
            .data
            .get_mut(offset..offset + 4)
            .ok_or_else(|| damaged(format!("write past the end at byte {offset}")))?;
        slot.copy_from_slice(&value.to_le_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::pdb::{device_sql_string, parse_pdb};

    fn track(id: u32, title: &str, artist: Option<&str>) -> TrackInput {
        TrackInput {
            id: Some(id),
            title: title.into(),
            file_path: format!("/Contents/{id:08X}/{title}.mp3"),
            artist: artist.map(str::to_string),
            album: Some("Album".into()),
            genre: Some("Techno".into()),
            key: Some("Am".into()),
            label: Some("Label".into()),
            analyze_path: usb_analysis_path(&format!("/Contents/{id:08X}/{title}.mp3")).unwrap(),
            tempo: 12_800 + id,
            duration: 300,
            bitrate: 320,
            file_size: 1000 * id,
            ..TrackInput::default()
        }
    }

    /// Row counts per table as rekordcrate, an independent reader, sees them.
    fn rekordcrate_rows(bytes: &[u8], page_type: rekordcrate::pdb::PageType) -> usize {
        use binrw::BinRead;
        use rekordcrate::pdb::Header;
        let mut cur = std::io::Cursor::new(bytes);
        let header = Header::read(&mut cur).unwrap();
        let t = header.tables.iter().find(|t| t.page_type == page_type).unwrap();
        let mut cur = std::io::Cursor::new(bytes);
        header
            .read_pages(&mut cur, binrw::Endian::NATIVE, (&t.first_page, &t.last_page))
            .unwrap()
            .into_iter()
            .flat_map(|p| p.row_groups.into_iter())
            .map(|g| g.present_rows().count())
            .sum()
    }

    #[test]
    fn template_decompresses_and_reads_as_an_empty_library() {
        let bytes = empty_device_pdb().unwrap();
        assert_eq!(bytes.len(), EMPTY_EXPORT_PDB_BYTES);
        let pdb = parse_pdb(&bytes).unwrap();
        assert_eq!(pdb.page_size, 4096);
        assert_eq!(pdb.table_count, 20);
        assert!(pdb.tracks.is_empty());
        assert!(pdb.playlists.is_empty());
        assert!(pdb.warnings.is_empty(), "{:?}", pdb.warnings);
        let inspection = inspect_pdb(&bytes).unwrap();
        assert!(inspection.tracks.is_empty() && inspection.playlists.is_empty() && inspection.histories.is_empty());
    }

    #[test]
    fn strings_use_rekordbox_encodings_and_read_back() {
        let long_ascii = "x".repeat(200);
        let cases = ["", "Short", &long_ascii, "Ääkkönen – Tïtle", "日本語"];
        for text in cases {
            let encoded = encode_device_sql_string(text).unwrap();
            let expected_kind = if !text.is_ascii() {
                0x90
            } else if text.len() > 126 {
                0x40
            } else {
                ((text.len() + 1) * 2 + 1) as u8
            };
            assert_eq!(encoded[0], expected_kind, "{text:?}");
            assert_eq!(decode_device_sql_string(&encoded, 0).unwrap(), text);
            assert_eq!(
                device_sql_string(&encoded, 0).as_deref(),
                Some(text),
                "BoothReady's reader disagrees on {text:?}"
            );
        }
        assert!(encode_device_sql_string(&"y".repeat(40_000)).is_err());
        assert!(decode_device_sql_string(&[0x50, 4, 0, 0], 0).is_err());
    }

    #[test]
    fn analysis_paths_match_djbooth() {
        // Expected values were produced by djbooth's legacyUsbAnalysisPath.
        let cases = [
            ("/Contents/00000001/track.mp3", "/PIONEER/USBANLZ/P016/00004354/ANLZ0000.DAT"),
            ("/Contents/0000002A/Some Track Title 42.mp3", "/PIONEER/USBANLZ/P061/00012433/ANLZ0000.DAT"),
            ("/Contents/00000003/Ääkkönen – Tïtle.flac", "/PIONEER/USBANLZ/P014/00009B40/ANLZ0000.DAT"),
            ("/Contents/00000004/日本語.wav", "/PIONEER/USBANLZ/P061/0001E02B/ANLZ0000.DAT"),
        ];
        for (audio, expected) in cases {
            assert_eq!(usb_analysis_path(audio).unwrap(), expected);
        }
        assert!(usb_analysis_path("/Music/track.mp3").is_err());
    }

    #[test]
    fn file_types_follow_the_extension() {
        assert_eq!(file_type_for_path("/Contents/1/a.MP3"), Some(1));
        assert_eq!(file_type_for_path("/Contents/1/a.aiff"), Some(12));
        assert_eq!(file_type_for_path("/Contents/1/a.flac"), Some(5));
        assert_eq!(file_type_for_path("/Contents/1/a.ogg"), None);
        assert_eq!(file_type_for_path("/Contents/1.d/a"), None);
    }

    #[test]
    fn round_trip_through_boothready_reader_and_rekordcrate() {
        let mut w = DevicePdbWriter::new().unwrap();
        let artists = ["Artist A", "Ääkkönen", "Artist A", "Long ".repeat(40).as_str().to_string().leak()];
        let mut ids = Vec::new();
        for i in 1..=300u32 {
            let artist = artists[(i as usize - 1) % artists.len()];
            let mut t = track(i, &format!("Track {i}"), Some(artist));
            if i == 7 {
                t.key = Some("F#m".into());
                t.album = Some("Other Album".into());
                t.genre = None;
                t.artist = None;
            }
            ids.push(w.add_track(&t).unwrap());
        }
        assert_eq!(ids, (1..=300).collect::<Vec<_>>());
        assert_eq!(w.track_count(), 300);
        let gigs = w.create_playlist("Gigs", 0, true).unwrap();
        let friday = w.create_playlist("Friday", gigs, false).unwrap();
        let warmup = w.create_playlist("Warm-up ☀", 0, false).unwrap();
        for id in (1..=300).rev() {
            w.add_to_playlist(friday, id, None).unwrap();
        }
        for id in 1..=5 {
            w.add_to_playlist(warmup, id, None).unwrap();
        }
        let bytes = w.build().unwrap();
        assert!(w.add_track(&track(301, "late", None)).is_err(), "finalized writers refuse rows");

        let pdb = parse_pdb(&bytes).unwrap();
        assert!(pdb.warnings.is_empty(), "{:?}", pdb.warnings);
        assert_eq!(pdb.sequence, parse_pdb(&empty_device_pdb().unwrap()).unwrap().sequence + 1);
        assert_eq!(pdb.tracks.len(), 300);
        let t42 = &pdb.tracks[41];
        assert_eq!(t42.id, 42);
        assert_eq!(t42.title, "Track 42");
        assert_eq!(t42.artist.as_deref(), Some("Ääkkönen"));
        assert_eq!(t42.file_path, "/Contents/0000002A/Track 42.mp3");
        assert_eq!(t42.analyze_path, usb_analysis_path(&t42.file_path).unwrap());
        assert_eq!((t42.sample_rate, t42.sample_depth, t42.bitrate, t42.file_size), (44_100, 16, 320, 42_000));
        assert_eq!((t42.duration_secs, t42.tempo), (300, 12_842));
        assert_eq!(pdb.tracks[6].artist, None);
        assert_eq!(pdb.tracks[3].artist.as_deref(), Some(artists[3]));
        let names: Vec<(&str, u32, bool)> =
            pdb.playlists.iter().map(|p| (p.name.as_str(), p.parent_id, p.is_folder)).collect();
        assert_eq!(names, vec![("Gigs", 0, true), ("Warm-up ☀", 0, false), ("Friday", gigs, false)]);
        assert_eq!(pdb.playlists[2].track_ids, (1..=300).rev().collect::<Vec<_>>());
        assert_eq!(pdb.playlists[1].track_ids, vec![1, 2, 3, 4, 5]);

        let inspection = inspect_pdb(&bytes).unwrap();
        assert_eq!(inspection.tracks.len(), 300);
        let t7 = &inspection.tracks[6];
        assert_eq!(
            (t7.key.as_deref(), t7.album.as_deref(), t7.genre.as_deref(), t7.artist.as_deref()),
            (Some("F#m"), Some("Other Album"), None, None)
        );
        assert_eq!(inspection.tracks[0].key.as_deref(), Some("Am"));
        assert_eq!(inspection.tracks[0].filename, "Track 1.mp3");
        assert_eq!(inspection.playlists.len(), 3);

        use rekordcrate::pdb::PageType;
        assert_eq!(rekordcrate_rows(&bytes, PageType::Tracks), 300);
        assert_eq!(rekordcrate_rows(&bytes, PageType::Artists), 3);
        assert_eq!(
            rekordcrate_rows(&bytes, PageType::Albums),
            4,
            "three artists with Album, plus Other Album without an artist"
        );
        assert_eq!(rekordcrate_rows(&bytes, PageType::Genres), 1);
        assert_eq!(rekordcrate_rows(&bytes, PageType::Keys), 2);
        assert_eq!(rekordcrate_rows(&bytes, PageType::Labels), 1);
        assert_eq!(rekordcrate_rows(&bytes, PageType::PlaylistTree), 3);
        assert_eq!(rekordcrate_rows(&bytes, PageType::PlaylistEntries), 305);
    }

    #[test]
    fn appending_to_an_existing_export_keeps_ids_and_shares_named_rows() {
        let mut w = DevicePdbWriter::new().unwrap();
        for i in 1..=3 {
            w.add_track(&track(i, &format!("T{i}"), Some("Same Artist"))).unwrap();
        }
        let list = w.create_playlist("First", 0, false).unwrap();
        w.add_to_playlist(list, 2, None).unwrap();
        let first = w.build().unwrap();

        let mut w = DevicePdbWriter::from_existing(&first).unwrap();
        assert_eq!(w.track_count(), 3);
        let dup = TrackInput {
            id: Some(2),
            title: "dup".into(),
            file_path: "/Contents/dup.mp3".into(),
            ..TrackInput::default()
        };
        assert!(w.add_track(&dup).is_err(), "existing IDs are taken");
        let new_id = w.add_track(&TrackInput { id: None, ..track(0, "T4", Some("Same Artist")) }).unwrap();
        assert_eq!(new_id, 4);
        let second = w.create_playlist("Second", 0, false).unwrap();
        assert_eq!(second, 2);
        w.add_to_playlist(list, 4, None).unwrap();
        w.add_to_playlist(second, 1, None).unwrap();
        let bytes = w.build().unwrap();

        let pdb = parse_pdb(&bytes).unwrap();
        assert!(pdb.warnings.is_empty(), "{:?}", pdb.warnings);
        assert_eq!(pdb.tracks.len(), 4);
        assert!(pdb.tracks.iter().all(|t| t.artist.as_deref() == Some("Same Artist")));
        assert_eq!(pdb.playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
        assert_eq!(pdb.playlists[0].track_ids, vec![2, 4]);
        assert_eq!(pdb.playlists[1].track_ids, vec![1]);
        assert_eq!(rekordcrate_rows(&bytes, rekordcrate::pdb::PageType::Artists), 1);
        assert_eq!(rekordcrate_rows(&bytes, rekordcrate::pdb::PageType::Albums), 1);
    }

    #[test]
    fn histories_are_written_and_cleared() {
        let mut w = DevicePdbWriter::new().unwrap();
        w.add_track(&track(1, "One", None)).unwrap();
        w.add_track(&track(2, "Two", None)).unwrap();
        let h = w.create_history("HISTORY 2026-10-18", None).unwrap();
        assert_eq!(h, 1);
        w.add_to_history(h, 2, None).unwrap();
        w.add_to_history(h, 1, None).unwrap();
        w.add_to_history(h, 2, None).unwrap();
        let bytes = w.build().unwrap();
        let inspection = inspect_pdb(&bytes).unwrap();
        assert_eq!(inspection.histories.len(), 1);
        assert_eq!(inspection.histories[0].name, "HISTORY 2026-10-18");
        assert_eq!(inspection.histories[0].track_ids, vec![2, 1, 2]);

        let mut w = DevicePdbWriter::from_existing(&bytes).unwrap();
        w.clear_histories().unwrap();
        let h2 = w.create_history("Later", None).unwrap();
        assert_eq!(h2, 1, "history IDs restart after clearing");
        let cleared = w.build().unwrap();
        let inspection = inspect_pdb(&cleared).unwrap();
        assert_eq!(inspection.histories.len(), 1);
        assert!(inspection.histories[0].track_ids.is_empty());
        assert_eq!(parse_pdb(&cleared).unwrap().tracks.len(), 2);
    }

    #[test]
    fn invalid_rows_are_refused() {
        let mut w = DevicePdbWriter::new().unwrap();
        assert!(w.add_track(&TrackInput { title: String::new(), ..track(1, "x", None) }).is_err());
        assert!(w.add_track(&TrackInput { file_path: "Contents/x.mp3".into(), ..track(1, "x", None) }).is_err());
        assert!(w.add_track(&TrackInput { file_path: "/Contents/x.ogg".into(), ..track(1, "x", None) }).is_err());
        assert!(w.add_track(&TrackInput { id: Some(0), ..track(1, "x", None) }).is_err());
        w.add_track(&track(1, "x", None)).unwrap();
        assert!(w.add_track(&track(1, "again", None)).is_err());
        let list = w.create_playlist("List", 0, false).unwrap();
        assert!(w.create_playlist("", 0, false).is_err());
        assert!(w.create_playlist("Child", list, false).is_err(), "a playlist cannot be a parent");
        assert!(w.create_playlist("Orphan", 99, false).is_err());
        let folder = w.create_playlist("Folder", 0, true).unwrap();
        assert!(w.add_to_playlist(folder, 1, None).is_err());
        assert!(w.add_to_playlist(list, 2, None).is_err(), "unknown track");
        assert!(w.add_to_history(1, 1, None).is_err());
        w.add_to_playlist(list, 1, None).unwrap();
    }

    #[test]
    fn thousands_of_rows_span_many_pages() {
        let mut w = DevicePdbWriter::new().unwrap();
        for i in 1..=1000 {
            w.add_track(&track(
                i,
                &format!("Track number {i} with a longer title"),
                Some(&format!("Artist {}", i % 50)),
            ))
            .unwrap();
        }
        let list = w.create_playlist("All", 0, false).unwrap();
        for i in 1..=1000 {
            w.add_to_playlist(list, i, None).unwrap();
        }
        let bytes = w.build().unwrap();
        assert_eq!(bytes.len() % 4096, 0);
        let pdb = parse_pdb(&bytes).unwrap();
        assert!(pdb.warnings.is_empty(), "{:?}", pdb.warnings);
        assert_eq!(pdb.tracks.len(), 1000);
        assert_eq!(pdb.tracks[999].artist.as_deref(), Some("Artist 0"));
        assert_eq!(pdb.playlists[0].track_ids, (1..=1000).collect::<Vec<_>>());
        assert_eq!(rekordcrate_rows(&bytes, rekordcrate::pdb::PageType::Tracks), 1000);
        assert_eq!(rekordcrate_rows(&bytes, rekordcrate::pdb::PageType::PlaylistEntries), 1000);
        let entry_pages = {
            use binrw::BinRead;
            let mut cur = std::io::Cursor::new(&bytes[..]);
            let header = rekordcrate::pdb::Header::read(&mut cur).unwrap();
            let t = header.tables.iter().find(|t| t.page_type == rekordcrate::pdb::PageType::PlaylistEntries).unwrap();
            let mut cur = std::io::Cursor::new(&bytes[..]);
            header.read_pages(&mut cur, binrw::Endian::NATIVE, (&t.first_page, &t.last_page)).unwrap().len()
        };
        assert_eq!(entry_pages, 5, "one index page and four data pages of 255 entries");
        assert_eq!(rekordcrate_rows(&bytes, rekordcrate::pdb::PageType::Artists), 50);
    }
}
