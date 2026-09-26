//! Local SQLite store: known USB drives, identity confirmations,
//! preparation history and saved gig profiles. Music files and library
//! metadata never go in here.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownMedia {
    /// Stable key: VID:PID:serial when available, else the manifest UUID.
    pub media_key: String,
    pub display_name: String,
    pub confirmed_catalog_id: Option<String>,
    pub nickname: Option<String>,
    pub role: Option<String>,
    pub first_seen_unix: u64,
    pub last_seen_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GigProfile {
    pub id: String,
    pub name: String,
    pub preset: Option<String>,
    pub targets: Vec<String>,
    pub playlists: Vec<String>,
    pub redundancy: bool,
    pub updated_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparationRecord {
    pub media_key: String,
    pub role: Option<String>,
    pub profile_id: Option<String>,
    pub started_unix: u64,
    pub finished_unix: Option<u64>,
    pub result: String,
}

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE known_media (
    media_key TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    confirmed_catalog_id TEXT,
    nickname TEXT,
    role TEXT,
    first_seen_unix INTEGER NOT NULL,
    last_seen_unix INTEGER NOT NULL
);
CREATE TABLE gig_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    body TEXT NOT NULL,
    updated_unix INTEGER NOT NULL
);
CREATE TABLE preparations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    media_key TEXT NOT NULL,
    role TEXT,
    profile_id TEXT,
    started_unix INTEGER NOT NULL,
    finished_unix INTEGER,
    result TEXT NOT NULL
);
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#];

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Store> {
        Store::init(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(m)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(Store { conn })
    }

    pub fn media_seen(&self, key: &str, display_name: &str, now: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO known_media (media_key, display_name, first_seen_unix, last_seen_unix) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(media_key) DO UPDATE SET last_seen_unix = ?3,
               display_name = CASE WHEN confirmed_catalog_id IS NULL THEN ?2 ELSE display_name END",
            params![key, display_name, now as i64],
        )?;
        Ok(())
    }

    pub fn confirm_identity(&self, key: &str, catalog_id: &str, display_name: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE known_media SET confirmed_catalog_id = ?2, display_name = ?3 WHERE media_key = ?1",
            params![key, catalog_id, display_name],
        )?;
        Ok(())
    }

    pub fn set_role(&self, key: &str, role: Option<&str>, nickname: Option<&str>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE known_media SET role = ?2, nickname = ?3 WHERE media_key = ?1",
            params![key, role, nickname],
        )?;
        Ok(())
    }

    pub fn media(&self, key: &str) -> rusqlite::Result<Option<KnownMedia>> {
        self.conn
            .query_row(
                "SELECT media_key, display_name, confirmed_catalog_id, nickname, role, first_seen_unix, last_seen_unix FROM known_media WHERE media_key = ?1",
                [key],
                row_to_media,
            )
            .optional()
    }

    pub fn known_media(&self) -> rusqlite::Result<Vec<KnownMedia>> {
        let mut st = self.conn.prepare(
            "SELECT media_key, display_name, confirmed_catalog_id, nickname, role, first_seen_unix, last_seen_unix FROM known_media ORDER BY last_seen_unix DESC",
        )?;
        let rows = st.query_map([], row_to_media)?;
        rows.collect()
    }

    pub fn save_profile(&self, p: &GigProfile) -> rusqlite::Result<()> {
        let body = serde_json::to_string(p).expect("profile serialises");
        self.conn.execute(
            "INSERT INTO gig_profiles (id, name, body, updated_unix) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = ?2, body = ?3, updated_unix = ?4",
            params![p.id, p.name, body, p.updated_unix as i64],
        )?;
        Ok(())
    }

    pub fn profiles(&self) -> rusqlite::Result<Vec<GigProfile>> {
        let mut st = self.conn.prepare("SELECT body FROM gig_profiles ORDER BY updated_unix DESC")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(Result::ok).filter_map(|b| serde_json::from_str(&b).ok()).collect())
    }

    pub fn delete_profile(&self, id: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM gig_profiles WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn start_preparation(&self, rec: &PreparationRecord) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO preparations (media_key, role, profile_id, started_unix, finished_unix, result) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rec.media_key, rec.role, rec.profile_id, rec.started_unix as i64, rec.finished_unix.map(|v| v as i64), rec.result],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_preparation(&self, id: i64, finished: u64, result: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE preparations SET finished_unix = ?2, result = ?3 WHERE id = ?1",
            params![id, finished as i64, result],
        )?;
        Ok(())
    }

    pub fn history(&self, key: &str) -> rusqlite::Result<Vec<PreparationRecord>> {
        let mut st = self.conn.prepare(
            "SELECT media_key, role, profile_id, started_unix, finished_unix, result FROM preparations WHERE media_key = ?1 ORDER BY id DESC",
        )?;
        let rows = st.query_map([key], |r| {
            Ok(PreparationRecord {
                media_key: r.get(0)?,
                role: r.get(1)?,
                profile_id: r.get(2)?,
                started_unix: r.get::<_, i64>(3)? as u64,
                finished_unix: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                result: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }
}

fn row_to_media(r: &rusqlite::Row<'_>) -> rusqlite::Result<KnownMedia> {
    Ok(KnownMedia {
        media_key: r.get(0)?,
        display_name: r.get(1)?,
        confirmed_catalog_id: r.get(2)?,
        nickname: r.get(3)?,
        role: r.get(4)?,
        first_seen_unix: r.get::<_, i64>(5)? as u64,
        last_seen_unix: r.get::<_, i64>(6)? as u64,
    })
}

/// Key a physical drive by its USB identity so it's recognised across
/// sessions even when the OS gives it a different device node.
pub fn media_key(dev: &boothready_model::PhysicalDevice) -> String {
    match &dev.usb {
        Some(u) if u.serial.as_deref().is_some_and(|s| !s.trim().is_empty()) => format!(
            "usb:{:04x}:{:04x}:{}",
            u.vendor_id.unwrap_or(0),
            u.product_id.unwrap_or(0),
            u.serial.as_deref().unwrap_or("").trim()
        ),
        _ => format!("dev:{}:{}", dev.raw_display_name(), dev.size_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_profiles_and_history() {
        let s = Store::in_memory().unwrap();
        s.media_seen("usb:0781:5581:ABC", "SanDisk Ultra USB 3.0 32 GB", 10).unwrap();
        s.confirm_identity("usb:0781:5581:ABC", "sandisk-ultra-usb3", "SanDisk Ultra 32 GB").unwrap();
        // A later sighting doesn't overwrite what the user confirmed.
        s.media_seen("usb:0781:5581:ABC", "raw name", 20).unwrap();
        let m = s.media("usb:0781:5581:ABC").unwrap().unwrap();
        assert_eq!(m.display_name, "SanDisk Ultra 32 GB");
        assert_eq!((m.first_seen_unix, m.last_seen_unix), (10, 20));

        let p = GigProfile {
            id: "fest".into(),
            name: "Festival unknown booth".into(),
            preset: Some("unknown_club".into()),
            targets: vec!["cdj-3000".into()],
            playlists: vec!["Peak".into()],
            redundancy: true,
            updated_unix: 5,
        };
        s.save_profile(&p).unwrap();
        assert_eq!(s.profiles().unwrap(), vec![p]);

        let id = s
            .start_preparation(&PreparationRecord {
                media_key: "usb:0781:5581:ABC".into(),
                role: Some("main".into()),
                profile_id: Some("fest".into()),
                started_unix: 30,
                finished_unix: None,
                result: "in_progress".into(),
            })
            .unwrap();
        s.finish_preparation(id, 40, "verified").unwrap();
        assert_eq!(s.history("usb:0781:5581:ABC").unwrap()[0].result, "verified");
    }

    #[test]
    fn reopening_keeps_data() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("br.sqlite");
        Store::open(&path).unwrap().set_setting("telemetry", "off").unwrap();
        assert_eq!(Store::open(&path).unwrap().setting("telemetry").unwrap().as_deref(), Some("off"));
    }
}
