//! Local cache: SQLite via rusqlite (architecture §8).
//!
//! Domain objects are stored as JSON blobs keyed by `(kind, id)`; small
//! settings (volume, shuffle, repeat, last screen) live in `kv`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

const SCHEMA: &str = include_str!("schema.sql");
pub const SCHEMA_VERSION: i64 = 1;
/// Upper bound on cached entities; oldest are pruned beyond this.
pub const MAX_ENTITIES: i64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Library,
    Home,
    Playlist,
    Album,
    Artist,
    Liked,
    User,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Home => "home",
            Self::Playlist => "playlist",
            Self::Album => "album",
            Self::Artist => "artist",
            Self::Liked => "liked",
            Self::User => "user",
        }
    }
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

fn now() -> i64 {
    crate::auth::oauth::now_secs() as i64
}

impl Store {
    /// Open (or create) the on-disk cache.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// In-memory cache for tests and `--mock`.
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| Error::Store("connection mutex poisoned".into()))?;
        f(&conn)
    }

    pub fn put<T: Serialize>(&self, kind: Kind, id: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.with(|c| {
            c.execute(
                "INSERT INTO entity(kind, id, json, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(kind, id) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
                params![kind.as_str(), id, json, now()],
            )?;
            Ok(())
        })
    }

    pub fn get<T: DeserializeOwned>(&self, kind: Kind, id: &str) -> Result<Option<T>> {
        let json: Option<String> = self.with(|c| {
            Ok(c.query_row(
                "SELECT json FROM entity WHERE kind = ?1 AND id = ?2",
                params![kind.as_str(), id],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        match json {
            // A schema change in the domain types just invalidates the row.
            Some(j) => Ok(serde_json::from_str(&j).ok()),
            None => Ok(None),
        }
    }

    pub fn count(&self) -> Result<i64> {
        self.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM entity", [], |r| r.get(0))?))
    }

    /// Delete the oldest entities beyond `max`.
    pub fn prune(&self, max: i64) -> Result<usize> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM entity WHERE rowid IN (
                    SELECT rowid FROM entity ORDER BY updated_at DESC, rowid DESC LIMIT -1 OFFSET ?1
                 )",
                params![max],
            )?)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO kv(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            Ok(
                c.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                    r.get(0)
                })
                .optional()?,
            )
        })
    }

    /// Remove all cached user data (logout).
    pub fn clear(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch("DELETE FROM entity; DELETE FROM kv;")?;
            Ok(())
        })
    }

    /// Run a store operation on the blocking pool.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|_| Error::Cancelled)?
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(Error::Store(format!(
            "cache was created by a newer Ferrotune (schema {version})"
        )));
    }
    if version < 1 {
        conn.execute_batch(SCHEMA)?;
    }
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock;
    use crate::net::model::Library;

    #[test]
    fn roundtrip_entities() {
        let s = Store::in_memory().unwrap();
        let lib = mock::library();
        s.put(Kind::Library, "me", &lib).unwrap();
        let back: Library = s.get(Kind::Library, "me").unwrap().unwrap();
        assert_eq!(back, lib);
        assert!(s.get::<Library>(Kind::Library, "other").unwrap().is_none());
        // Overwrite keeps one row.
        s.put(Kind::Library, "me", &lib).unwrap();
        assert_eq!(s.count().unwrap(), 1);
    }

    #[test]
    fn incompatible_json_is_a_miss() {
        let s = Store::in_memory().unwrap();
        s.put(Kind::Album, "x", &"just a string").unwrap();
        assert!(s.get::<Library>(Kind::Album, "x").unwrap().is_none());
    }

    #[test]
    fn settings_and_clear() {
        let s = Store::in_memory().unwrap();
        s.set_setting("volume", "0.5").unwrap();
        s.set_setting("volume", "0.7").unwrap();
        assert_eq!(s.setting("volume").unwrap().as_deref(), Some("0.7"));
        s.put(Kind::Home, "me", &1).unwrap();
        s.clear().unwrap();
        assert_eq!(s.setting("volume").unwrap(), None);
        assert_eq!(s.count().unwrap(), 0);
    }

    #[test]
    fn prune_keeps_newest() {
        let s = Store::in_memory().unwrap();
        for i in 0..10 {
            s.put(Kind::Album, &i.to_string(), &i).unwrap();
        }
        assert_eq!(s.prune(4).unwrap(), 6);
        assert_eq!(s.count().unwrap(), 4);
        assert_eq!(s.get::<i32>(Kind::Album, "9").unwrap(), Some(9));
    }

    #[test]
    fn migration_is_idempotent_and_versioned() {
        let dir = std::env::temp_dir().join(format!("ferrotune-store-{}", std::process::id()));
        let path = dir.join("cache.sqlite3");
        {
            let s = Store::open(&path).unwrap();
            s.set_setting("k", "v").unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.setting("k").unwrap().as_deref(), Some("v"));
        let v: i64 = s
            .with(|c| Ok(c.pragma_query_value(None, "user_version", |r| r.get(0))?))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn async_run() {
        let s = Store::in_memory().unwrap();
        s.run(|s| s.set_setting("a", "b")).await.unwrap();
        let v = s.run(|s| s.setting("a")).await.unwrap();
        assert_eq!(v.as_deref(), Some("b"));
    }
}
