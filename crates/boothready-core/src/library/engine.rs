//! Engine DJ library (`Engine Library/Database2/m.db`) validation.
//!
//! The database is plain SQLite. We open it read-only and immutable so that
//! SQLite never creates `-wal`/`-shm` files on the user's drive.

use super::{PathResolver, PlaylistSummary, ENGINE_DB_FILE, ENGINE_LEGACY_DB_FILE};
#[cfg(feature = "sqlite")]
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
#[cfg(feature = "sqlite")]
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineReport {
    /// Engine Prime 1.x layout (`Engine Library/m.db`).
    pub legacy: bool,
    pub schema_version: Option<String>,
    pub track_count: u32,
    pub playlists: Vec<PlaylistSummary>,
    /// Volume-relative paths of tracks stored on this drive.
    pub track_paths: Vec<String>,
    pub missing: Vec<String>,
    /// Tracks that point at a computer's disk rather than this drive.
    pub outside_drive: u32,
    pub error: Option<String>,
}

pub(super) fn scan(resolver: &mut PathResolver) -> Option<EngineReport> {
    let (rel, legacy) = if let Some(_p) = resolver.resolve(ENGINE_DB_FILE) {
        (ENGINE_DB_FILE, false)
    } else if resolver.resolve(ENGINE_LEGACY_DB_FILE).is_some() {
        (ENGINE_LEGACY_DB_FILE, true)
    } else {
        return None;
    };
    let path = resolver.resolve(rel)?;
    let mut report = EngineReport {
        legacy,
        schema_version: None,
        track_count: 0,
        playlists: vec![],
        track_paths: vec![],
        missing: vec![],
        outside_drive: 0,
        error: None,
    };
    #[cfg(feature = "sqlite")]
    if let Err(e) = read(&path, rel, resolver, &mut report) {
        report.error = Some(e.to_string());
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = path;
        report.error = Some("Engine DJ support is not included in this build".into());
    }
    Some(report)
}

#[cfg(feature = "sqlite")]
fn open_readonly(path: &Path) -> rusqlite::Result<Connection> {
    let uri = format!("file:{}?mode=ro&immutable=1", uri_escape(&path.to_string_lossy()));
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// SQLite URI for a local file. Only `%`, `?` and `#` need escaping; Windows
/// paths become `file:///C:/...`.
#[cfg(feature = "sqlite")]
fn uri_escape(s: &str) -> String {
    let s = s.replace('\\', "/").replace('%', "%25").replace('?', "%3F").replace('#', "%23");
    let b = s.as_bytes();
    if b.len() > 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        format!("///{s}")
    } else {
        s
    }
}

#[cfg(feature = "sqlite")]
fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(())).is_ok()
}

#[cfg(feature = "sqlite")]
fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name=?1");
    conn.query_row(&sql, [column], |_| Ok(())).is_ok()
}

#[cfg(feature = "sqlite")]
fn read(path: &Path, rel: &str, resolver: &mut PathResolver, report: &mut EngineReport) -> rusqlite::Result<()> {
    let conn = open_readonly(path)?;
    if has_table(&conn, "Information") && has_column(&conn, "Information", "schemaVersionMajor") {
        report.schema_version = conn
            .query_row(
                "SELECT schemaVersionMajor, schemaVersionMinor, schemaVersionPatch FROM Information LIMIT 1",
                [],
                |r| Ok(format!("{}.{}.{}", r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
            )
            .ok();
    }
    if !has_table(&conn, "Track") {
        report.error = Some("database has no Track table".into());
        return Ok(());
    }
    // Relative paths in Engine databases are written from the database's
    // point of view. Accept any of the plausible bases rather than guess one.
    let db_dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let engine_dir = db_dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let bases = [db_dir, engine_dir, ""];
    let mut stmt = conn.prepare("SELECT path FROM Track WHERE path IS NOT NULL AND path != ''")?;
    let paths = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for p in paths {
        let p = p?;
        report.track_count += 1;
        if is_absolute_elsewhere(&p) {
            report.outside_drive += 1;
            continue;
        }
        let found = bases.iter().filter_map(|b| normalize(b, &p)).find(|cand| resolver.resolve(cand).is_some());
        match found {
            Some(c) => report.track_paths.push(c),
            None => report.missing.push(normalize(db_dir, &p).unwrap_or(p)),
        }
    }
    if has_table(&conn, "Playlist") && has_column(&conn, "Playlist", "parentListId") {
        let mut stmt = conn.prepare("SELECT id, title, parentListId FROM Playlist")?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaylistSummary {
                id: r.get::<_, i64>(0)? as u32,
                name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                parent_id: r.get::<_, Option<i64>>(2)?.unwrap_or(0) as u32,
                is_folder: false,
                track_ids: vec![],
            })
        })?;
        for row in rows {
            report.playlists.push(row?);
        }
        if has_table(&conn, "PlaylistEntity") {
            let mut stmt = conn.prepare("SELECT listId, trackId FROM PlaylistEntity ORDER BY listId, id")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32)))?;
            for row in rows {
                let (list, track) = row?;
                if let Some(pl) = report.playlists.iter_mut().find(|p| p.id == list) {
                    pl.track_ids.push(track);
                }
            }
        }
        let parents: std::collections::HashSet<u32> = report.playlists.iter().map(|p| p.parent_id).collect();
        for pl in &mut report.playlists {
            pl.is_folder = parents.contains(&pl.id) && pl.track_ids.is_empty();
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
fn is_absolute_elsewhere(p: &str) -> bool {
    p.starts_with("/Users/")
        || p.starts_with("/Volumes/")
        || p.starts_with("/home/")
        || (p.len() > 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic())
}

/// Join `path` onto the volume-relative directory `base`, resolving `..`.
/// Returns `None` if the result would escape the volume.
#[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
pub(crate) fn normalize(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> =
        if path.starts_with('/') { vec![] } else { base.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in path.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalize_paths() {
        assert_eq!(
            normalize("Engine Library/Database2", "../Music/a.mp3").as_deref(),
            Some("Engine Library/Music/a.mp3")
        );
        assert_eq!(normalize("Engine Library/Database2", "../../Music/a.mp3").as_deref(), Some("Music/a.mp3"));
        assert_eq!(normalize("Engine Library", "../../../etc/passwd"), None);
        assert_eq!(normalize("x", "/Contents/b.mp3").as_deref(), Some("Contents/b.mp3"));
    }
}
