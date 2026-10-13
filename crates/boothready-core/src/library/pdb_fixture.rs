//! Builds small, structurally valid `export.pdb` files for tests and demo
//! images. Not used by the product at runtime: BoothReady does not write
//! vendor databases.

use super::pdb::{TABLE_ARTISTS, TABLE_PLAYLIST_ENTRIES, TABLE_PLAYLIST_TREE, TABLE_TRACKS};

const PAGE: usize = 4096;
const HEAP: usize = 0x28;
const GROUP: usize = 0x24;

#[derive(Debug, Clone)]
pub struct FixtureTrack {
    pub id: u32,
    pub title: String,
    pub artist_id: u32,
    pub file_path: String,
    pub sample_rate: u32,
    pub sample_depth: u16,
    pub bitrate: u32,
    pub file_size: u32,
    pub duration_secs: u16,
    pub tempo: u32,
}

#[derive(Debug, Clone)]
pub struct FixturePlaylist {
    pub id: u32,
    pub parent_id: u32,
    pub name: String,
    pub is_folder: bool,
    pub sort_order: u32,
    pub track_ids: Vec<u32>,
}

/// Encode a DeviceSQL string the way rekordbox does: short ASCII when it
/// fits, UTF-16LE otherwise.
pub fn encode_string(s: &str) -> Vec<u8> {
    if s.is_ascii() && s.len() <= 126 {
        let mut v = vec![(((s.len() + 1) << 1) | 1) as u8];
        v.extend_from_slice(s.as_bytes());
        v
    } else {
        let units: Vec<u16> = s.encode_utf16().collect();
        let len = 4 + units.len() * 2;
        let mut v = vec![0x90];
        v.extend_from_slice(&(len as u16).to_le_bytes());
        v.push(0);
        for u in units {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }
}

fn track_row(t: &FixtureTrack) -> Vec<u8> {
    let mut r = vec![0u8; 0x88];
    r[0..2].copy_from_slice(&0x24u16.to_le_bytes());
    r[0x08..0x0C].copy_from_slice(&t.sample_rate.to_le_bytes());
    r[0x10..0x14].copy_from_slice(&t.file_size.to_le_bytes());
    r[0x30..0x34].copy_from_slice(&t.bitrate.to_le_bytes());
    r[0x38..0x3C].copy_from_slice(&t.tempo.to_le_bytes());
    r[0x44..0x48].copy_from_slice(&t.artist_id.to_le_bytes());
    r[0x48..0x4C].copy_from_slice(&t.id.to_le_bytes());
    r[0x52..0x54].copy_from_slice(&t.sample_depth.to_le_bytes());
    r[0x54..0x56].copy_from_slice(&t.duration_secs.to_le_bytes());
    r[0x56..0x58].copy_from_slice(&0x29u16.to_le_bytes());
    r[0x5A..0x5C].copy_from_slice(&1u16.to_le_bytes());
    let filename = t.file_path.rsplit('/').next().unwrap_or("").to_string();
    let analyze = format!("/PIONEER/USBANLZ/P000/{:08X}/ANLZ0000.DAT", t.id);
    for i in 0..21 {
        let s = match i {
            14 => analyze.clone(),
            17 => t.title.clone(),
            19 => filename.clone(),
            20 => t.file_path.clone(),
            _ => String::new(),
        };
        let ofs = r.len() as u16;
        r[0x5E + 2 * i..0x60 + 2 * i].copy_from_slice(&ofs.to_le_bytes());
        r.extend_from_slice(&encode_string(&s));
    }
    r
}

fn artist_row(id: u32, name: &str) -> Vec<u8> {
    let mut r = vec![0u8; 10];
    r[0..2].copy_from_slice(&0x60u16.to_le_bytes());
    r[4..8].copy_from_slice(&id.to_le_bytes());
    r[8] = 0x03;
    r[9] = 10;
    r.extend_from_slice(&encode_string(name));
    r
}

fn playlist_row(p: &FixturePlaylist) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend_from_slice(&p.parent_id.to_le_bytes());
    r.extend_from_slice(&0u32.to_le_bytes());
    r.extend_from_slice(&p.sort_order.to_le_bytes());
    r.extend_from_slice(&p.id.to_le_bytes());
    r.extend_from_slice(&(p.is_folder as u32).to_le_bytes());
    r.extend_from_slice(&encode_string(&p.name));
    r
}

fn entry_row(index: u32, track: u32, playlist: u32) -> Vec<u8> {
    let mut r = Vec::new();
    r.extend_from_slice(&index.to_le_bytes());
    r.extend_from_slice(&track.to_le_bytes());
    r.extend_from_slice(&playlist.to_le_bytes());
    r
}

/// Pack rows into as many data pages as needed.
fn pack_pages(rows: &[Vec<u8>]) -> Vec<Vec<Vec<u8>>> {
    let mut pages: Vec<Vec<Vec<u8>>> = Vec::new();
    let mut current: Vec<Vec<u8>> = Vec::new();
    let mut used = 0usize;
    for row in rows {
        let len = row.len().div_ceil(4) * 4;
        let n = current.len() + 1;
        if HEAP + used + len + n.div_ceil(16) * GROUP > PAGE {
            pages.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(row.clone());
        used += len;
    }
    pages.push(current);
    pages
}

fn write_page(buf: &mut [u8], index: u32, kind: u32, next: u32, rows: &[Vec<u8>], index_page: bool) {
    buf[4..8].copy_from_slice(&index.to_le_bytes());
    buf[8..12].copy_from_slice(&kind.to_le_bytes());
    buf[12..16].copy_from_slice(&next.to_le_bytes());
    buf[16..20].copy_from_slice(&1u32.to_le_bytes());
    if index_page {
        buf[0x1B] = 0x64;
        return;
    }
    let n = rows.len();
    let packed = (n as u32 & 0x1FFF) | ((n as u32) << 13);
    buf[0x18..0x1B].copy_from_slice(&packed.to_le_bytes()[..3]);
    buf[0x1B] = 0x34;
    buf[0x22..0x24].copy_from_slice(&(n as u16).to_le_bytes());
    let mut heap = 0usize;
    for (i, row) in rows.iter().enumerate() {
        buf[HEAP + heap..HEAP + heap + row.len()].copy_from_slice(row);
        let g = i / 16;
        let j = i % 16;
        let base = PAGE - g * GROUP;
        buf[base - 6 - 2 * j..base - 4 - 2 * j].copy_from_slice(&(heap as u16).to_le_bytes());
        let flags_at = base - 4;
        let flags = u16::from_le_bytes([buf[flags_at], buf[flags_at + 1]]) | (1 << j);
        buf[flags_at..flags_at + 2].copy_from_slice(&flags.to_le_bytes());
        heap += row.len().div_ceil(4) * 4;
    }
    let free = PAGE - HEAP - heap - n.div_ceil(16) * GROUP;
    buf[0x1C..0x1E].copy_from_slice(&(free as u16).to_le_bytes());
    buf[0x1E..0x20].copy_from_slice(&(heap as u16).to_le_bytes());
}

/// Build an `export.pdb` holding the given tracks, artists and playlists.
pub fn build_pdb(tracks: &[FixtureTrack], artists: &[(u32, String)], playlists: &[FixturePlaylist]) -> Vec<u8> {
    let track_rows: Vec<Vec<u8>> = tracks.iter().map(track_row).collect();
    let artist_rows: Vec<Vec<u8>> = artists.iter().map(|(id, n)| artist_row(*id, n)).collect();
    let playlist_rows: Vec<Vec<u8>> = playlists.iter().map(playlist_row).collect();
    let entry_rows: Vec<Vec<u8>> = playlists
        .iter()
        .flat_map(|p| p.track_ids.iter().enumerate().map(move |(i, &t)| entry_row(i as u32 + 1, t, p.id)))
        .collect();
    let tables = [
        (TABLE_TRACKS, track_rows),
        (TABLE_ARTISTS, artist_rows),
        (TABLE_PLAYLIST_TREE, playlist_rows),
        (TABLE_PLAYLIST_ENTRIES, entry_rows),
    ];

    let mut pages: Vec<Vec<u8>> = vec![vec![0u8; PAGE]];
    let mut pointers = Vec::new();
    for (kind, rows) in &tables {
        let packed = pack_pages(rows);
        let index_page = pages.len() as u32;
        let first_data = index_page + 1;
        let last_data = index_page + packed.len() as u32;
        let empty_candidate = last_data + 1;
        let mut ip = vec![0u8; PAGE];
        write_page(&mut ip, index_page, *kind, first_data, &[], true);
        pages.push(ip);
        for (i, page_rows) in packed.iter().enumerate() {
            let idx = first_data + i as u32;
            let next = if idx == last_data { empty_candidate } else { idx + 1 };
            let mut p = vec![0u8; PAGE];
            write_page(&mut p, idx, *kind, next, page_rows, false);
            pages.push(p);
        }
        pointers.push((*kind, empty_candidate, index_page, last_data));
    }
    let next_unused = pages.len() as u32 + tables.len() as u32;
    let header = &mut pages[0];
    header[4..8].copy_from_slice(&(PAGE as u32).to_le_bytes());
    header[8..12].copy_from_slice(&(tables.len() as u32).to_le_bytes());
    header[12..16].copy_from_slice(&next_unused.to_le_bytes());
    header[20..24].copy_from_slice(&7u32.to_le_bytes());
    for (i, (kind, empty, first, last)) in pointers.iter().enumerate() {
        let o = 0x1C + i * 16;
        header[o..o + 4].copy_from_slice(&kind.to_le_bytes());
        header[o + 4..o + 8].copy_from_slice(&empty.to_le_bytes());
        header[o + 8..o + 12].copy_from_slice(&first.to_le_bytes());
        header[o + 12..o + 16].copy_from_slice(&last.to_le_bytes());
    }
    pages.concat()
}
