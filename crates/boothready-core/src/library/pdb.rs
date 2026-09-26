//! Read-only parser for rekordbox "Device Library" databases (`export.pdb`).
//!
//! This is a clean-room implementation written from the public format
//! analysis published by Deep Symmetry (crate-digger) and the rekordcrate
//! project. It only *reads*; BoothReady never writes vendor databases.
//!
//! Layout recap: the file is a sequence of fixed-size pages. Page 0 holds a
//! header with one pointer per table. Each table is a linked list of pages.
//! Rows live in a heap that grows forward from offset 0x28 of each page, and
//! their offsets are stored in groups of 16 that grow backwards from the end
//! of the page, with a bitmask saying which rows are present.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const TABLE_TRACKS: u32 = 0;
pub const TABLE_GENRES: u32 = 1;
pub const TABLE_ARTISTS: u32 = 2;
pub const TABLE_ALBUMS: u32 = 3;
pub const TABLE_KEYS: u32 = 5;
pub const TABLE_PLAYLIST_TREE: u32 = 7;
pub const TABLE_PLAYLIST_ENTRIES: u32 = 8;

const PAGE_HEADER_LEN: usize = 0x28;
const ROW_GROUP_LEN: usize = 0x24;
/// Refuse to load absurd files into memory.
pub const MAX_PDB_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdbTrack {
    pub id: u32,
    pub title: String,
    pub artist: Option<String>,
    /// Absolute path from the volume root, e.g. `/Contents/Artist/Album/x.mp3`.
    pub file_path: String,
    pub sample_rate: u32,
    pub sample_depth: u16,
    pub bitrate: u32,
    pub file_size: u32,
    pub duration_secs: u16,
    /// BPM × 100.
    pub tempo: u32,
    pub analyze_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdbPlaylist {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: u32,
    /// Track IDs in playlist order.
    pub track_ids: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pdb {
    pub page_size: u32,
    pub sequence: u32,
    pub table_count: u32,
    pub tracks: Vec<PdbTrack>,
    pub playlists: Vec<PdbPlaylist>,
    /// Row counts per table type, for diagnostics.
    pub row_counts: Vec<(u32, u32)>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PdbError {
    #[error("file is too small to be a rekordbox database")]
    TooSmall,
    #[error("header is not a rekordbox database header")]
    BadHeader,
    #[error("unsupported page size {0}")]
    BadPageSize(u32),
}

#[derive(Debug, Clone, Copy)]
struct TablePointer {
    kind: u32,
    first_page: u32,
    last_page: u32,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

/// Decode a DeviceSQL string at `o`. Returns `None` for malformed strings.
pub(crate) fn device_sql_string(b: &[u8], o: usize) -> Option<String> {
    let kind = *b.get(o)?;
    match kind {
        0x40 => {
            let len = u16_at(b, o + 1)? as usize;
            let text = b.get(o + 4..o + len.max(4))?;
            Some(String::from_utf8_lossy(text).into_owned())
        }
        0x90 => {
            let len = u16_at(b, o + 1)? as usize;
            let text = b.get(o + 4..o + len.max(4))?;
            let units: Vec<u16> = text.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
        }
        k if k & 1 == 1 => {
            let len = (k >> 1) as usize;
            let text = b.get(o + 1..o + len.max(1))?;
            Some(String::from_utf8_lossy(text).into_owned())
        }
        _ => None,
    }
}

pub fn parse_pdb(data: &[u8]) -> Result<Pdb, PdbError> {
    if data.len() < 0x1C + 16 {
        return Err(PdbError::TooSmall);
    }
    if u32_at(data, 0) != Some(0) {
        return Err(PdbError::BadHeader);
    }
    let page_size = u32_at(data, 4).unwrap();
    if !matches!(page_size, 512 | 1024 | 2048 | 4096 | 8192 | 16384) {
        return Err(PdbError::BadPageSize(page_size));
    }
    let num_tables = u32_at(data, 8).unwrap();
    let sequence = u32_at(data, 20).unwrap();
    if num_tables == 0 || num_tables > 64 || 0x1C + num_tables as usize * 16 > page_size as usize {
        return Err(PdbError::BadHeader);
    }
    let mut tables = Vec::new();
    for i in 0..num_tables as usize {
        let o = 0x1C + i * 16;
        tables.push(TablePointer {
            kind: u32_at(data, o).unwrap(),
            first_page: u32_at(data, o + 8).unwrap(),
            last_page: u32_at(data, o + 12).unwrap(),
        });
    }

    let mut pdb = Pdb { page_size, sequence, table_count: num_tables, ..Default::default() };
    let reader = Reader { data, page_size: page_size as usize };

    let mut artists: HashMap<u32, String> = HashMap::new();
    let mut track_artist: Vec<u32> = Vec::new();
    let mut tree: Vec<PdbPlaylist> = Vec::new();
    let mut entries: Vec<(u32, u32, u32)> = Vec::new();

    for t in &tables {
        let rows = reader.table_rows(t, &mut pdb.warnings);
        pdb.row_counts.push((t.kind, rows.len() as u32));
        match t.kind {
            TABLE_TRACKS => {
                for r in rows {
                    match parse_track(data, r) {
                        Some((tr, artist_id)) => {
                            pdb.tracks.push(tr);
                            track_artist.push(artist_id);
                        }
                        None => pdb.warnings.push(format!("unreadable track row at byte {r}")),
                    }
                }
            }
            TABLE_ARTISTS => {
                for r in rows {
                    if let Some((id, name)) = parse_artist(data, r) {
                        artists.insert(id, name);
                    }
                }
            }
            TABLE_PLAYLIST_TREE => {
                for r in rows {
                    if let Some(p) = parse_playlist_node(data, r) {
                        tree.push(p);
                    } else {
                        pdb.warnings.push(format!("unreadable playlist row at byte {r}"));
                    }
                }
            }
            TABLE_PLAYLIST_ENTRIES => {
                for r in rows {
                    if let (Some(idx), Some(track), Some(pl)) = (u32_at(data, r), u32_at(data, r + 4), u32_at(data, r + 8)) {
                        entries.push((pl, idx, track));
                    }
                }
            }
            _ => {}
        }
    }

    entries.sort_unstable();
    let mut by_playlist: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pl, _, track) in entries {
        by_playlist.entry(pl).or_default().push(track);
    }
    for p in &mut tree {
        p.track_ids = by_playlist.remove(&p.id).unwrap_or_default();
    }
    tree.sort_by_key(|p| (p.parent_id, p.sort_order, p.id));
    pdb.playlists = tree;
    for (t, artist_id) in pdb.tracks.iter_mut().zip(track_artist) {
        t.artist = artists.get(&artist_id).cloned().filter(|a| !a.is_empty());
    }
    let known: HashSet<u32> = pdb.tracks.iter().map(|t| t.id).collect();
    let dangling = pdb.playlists.iter().flat_map(|p| p.track_ids.iter()).filter(|id| !known.contains(id)).count();
    if dangling > 0 {
        pdb.warnings.push(format!("{dangling} playlist entries point at tracks that are not in the database"));
    }
    Ok(pdb)
}

struct Reader<'a> {
    data: &'a [u8],
    page_size: usize,
}

impl Reader<'_> {
    fn page(&self, index: u32) -> Option<&[u8]> {
        let start = (index as usize).checked_mul(self.page_size)?;
        self.data.get(start..start.checked_add(self.page_size)?)
    }

    /// Absolute byte offsets of every present row in the table.
    fn table_rows(&self, t: &TablePointer, warnings: &mut Vec<String>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        let mut index = t.first_page;
        let max_pages = self.data.len() / self.page_size;
        loop {
            if !visited.insert(index) || visited.len() > max_pages {
                warnings.push(format!("table {} has a page loop", t.kind));
                break;
            }
            let Some(page) = self.page(index) else {
                warnings.push(format!("table {} points past the end of the file (page {index})", t.kind));
                break;
            };
            let page_start = index as usize * self.page_size;
            let page_type = u32_at(page, 8).unwrap();
            let next = u32_at(page, 12).unwrap();
            let flags = page[0x1B];
            let is_data = flags & 0x40 == 0;
            if page_type == t.kind && is_data {
                self.page_rows(page, page_start, &mut out);
            }
            if index == t.last_page {
                break;
            }
            index = next;
        }
        out
    }

    fn page_rows(&self, page: &[u8], page_start: usize, out: &mut Vec<usize>) {
        let small = page[0x18] as u16;
        let large = u16_at(page, 0x22).unwrap();
        let num_rows = if large > small && large != 0x1FFF { large } else { small } as usize;
        if num_rows == 0 {
            return;
        }
        let groups = num_rows.div_ceil(16);
        if groups * ROW_GROUP_LEN + PAGE_HEADER_LEN > self.page_size {
            return;
        }
        for g in 0..groups {
            let base = self.page_size - g * ROW_GROUP_LEN;
            let flags = u16_at(page, base - 4).unwrap();
            for j in 0..16 {
                if flags & (1 << j) == 0 {
                    continue;
                }
                let ofs = u16_at(page, base - 6 - 2 * j).unwrap() as usize;
                let row = PAGE_HEADER_LEN + ofs;
                if row < self.page_size {
                    out.push(page_start + row);
                }
            }
        }
    }
}

fn parse_track(data: &[u8], r: usize) -> Option<(PdbTrack, u32)> {
    let row = data.get(r..)?;
    if row.len() < 0x88 {
        return None;
    }
    let s = |i: usize| -> Option<String> {
        let ofs = u16_at(row, 0x5E + 2 * i)? as usize;
        device_sql_string(row, ofs)
    };
    let artist_id = u32_at(row, 0x44)?;
    let track = PdbTrack {
        sample_rate: u32_at(row, 0x08)?,
        file_size: u32_at(row, 0x10)?,
        bitrate: u32_at(row, 0x30)?,
        tempo: u32_at(row, 0x38)?,
        artist: None,
        id: u32_at(row, 0x48)?,
        sample_depth: u16_at(row, 0x52)?,
        duration_secs: u16_at(row, 0x54)?,
        analyze_path: s(14).unwrap_or_default(),
        title: s(17).unwrap_or_default(),
        file_path: s(20)?,
    };
    Some((track, artist_id))
}

fn parse_artist(data: &[u8], r: usize) -> Option<(u32, String)> {
    let row = data.get(r..)?;
    let subtype = u16_at(row, 0)?;
    let id = u32_at(row, 4)?;
    let ofs = if subtype == 0x64 { u16_at(row, 0x0A)? as usize } else { *row.get(9)? as usize };
    Some((id, device_sql_string(row, ofs)?))
}

fn parse_playlist_node(data: &[u8], r: usize) -> Option<PdbPlaylist> {
    let row = data.get(r..)?;
    Some(PdbPlaylist {
        parent_id: u32_at(row, 0)?,
        sort_order: u32_at(row, 8)?,
        id: u32_at(row, 12)?,
        is_folder: u32_at(row, 16)? != 0,
        name: device_sql_string(row, 20)?,
        track_ids: vec![],
    })
}
